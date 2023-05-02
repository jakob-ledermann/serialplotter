use std::{
    io::{self},
    thread,
};

use crossbeam::channel::{Receiver, SendError, Sender};

#[derive(Debug, PartialEq, Clone)]
pub struct DataValue {
    pub name: String,
    pub value: f64,
}

use serialport::SerialPort;
use tracing::{info, warn};

use crate::value_parsing::parsing_state_machine::{Parser, ParsingResult};

pub struct SerialSource {}

#[allow(dead_code)]
pub enum Commands {
    Stop,
    SendMessage(String),
}

impl SerialSource {
    pub fn start(
        port: Box<dyn SerialPort>,
        datasender: Sender<DataValue>,
        command_receiver: Receiver<Commands>,
    ) {
        info!("Start reading from {:?}", port.name());
        let _thread = thread::Builder::new()
            .name(format!("Read serial {}", port.name().unwrap()))
            .spawn(move || process_serial_data(port, datasender, command_receiver));
    }
}

fn process_serial_data(
    mut port: Box<dyn SerialPort>,
    datasender: Sender<DataValue>,
    command_receiver: Receiver<Commands>,
) {
    #[cfg(feature = "profiling")]
    {
        puffin::set_scopes_on(true);
        puffin::profile_scope!("processing serial data");
    }

    let span = tracing::span!(tracing::Level::DEBUG, "Processing Serialport");
    let _scope = span.enter();
    let name = port.name();
    info!(
        "Start reading from {:?} with timeout {:?}",
        &name,
        port.timeout()
    );
    let mut line = String::new();
    let mut buffer = [0u8; 1024];
    let _offset = 0;
    let _minimum_message_size = buffer.len();
    let mut parser = Parser::new();
    'read_loop: loop {
        if let Ok(command) = command_receiver.try_recv() {
            match command {
                Commands::Stop => break 'read_loop,
                Commands::SendMessage(message) => port
                    .write(message.as_bytes())
                    .expect("should be able to write to the port"),
            };
        }
        line.clear();
        let available = port.bytes_to_read().unwrap();
        let result = port.read(&mut buffer[..usize::try_from(available.clamp(1, 1024)).unwrap()]);
        {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("processing received data");
            let result = match result {
                Ok(amount) => {
                    for byte in &buffer[..amount] {
                        let result = parser.parse(*byte);
                        match result {
                            ParsingResult::Pending => {}
                            ParsingResult::Err(err) => {
                                warn!("error parsing value {:?}", err)
                            }
                            ParsingResult::Ok(values) => {
                                for value in values {
                                    datasender.send(value).unwrap();
                                }
                            }
                        }
                    }
                    Ok(())
                }
                Err(err) => match err.kind() {
                    io::ErrorKind::Interrupted => Ok(()),
                    io::ErrorKind::WouldBlock => Ok(()),
                    _ => {
                        warn!("Error reading from buffer: {}", err);
                        Err(ParseError::ChannelClosed)
                    }
                },
            };
            match result {
                Ok(_) | Err(ParseError::InvalidFormat) => {}
                Err(ParseError::ChannelClosed) => break,
            }
        }
    }
    info!("Stop reading from {:?}", &name);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParseError {
    ChannelClosed,
    InvalidFormat,
}

impl From<SendError<DataValue>> for ParseError {
    fn from(_value: SendError<DataValue>) -> Self {
        Self::ChannelClosed
    }
}
mod parsing_state_machine {
    use std::mem;

    use super::{DataValue, ParseError};

    #[derive(Debug, Clone, PartialEq)]
    pub enum ParsingResult {
        Ok(Vec<DataValue>),
        Pending,
        Err(ParseError),
    }

    impl From<Result<Vec<DataValue>, ParseError>> for ParsingResult {
        fn from(other: Result<Vec<DataValue>, ParseError>) -> Self {
            match other {
                Ok(values) => ParsingResult::Ok(values),
                Err(err) => ParsingResult::Err(err),
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct Parser {
        name: Option<String>,
        value: String,
        completed_values: Vec<DataValue>,
    }

    impl Parser {
        pub fn new() -> Self {
            Self {
                name: None,
                value: String::with_capacity(10),
                completed_values: Vec::new(),
            }
        }

        pub fn parse(&mut self, byte: u8) -> ParsingResult {
            match byte {
                b'\n' => ParsingResult::from(self.finish()),
                b',' => match self.complete_value() {
                    Ok(()) => ParsingResult::Pending,
                    Err(err) => ParsingResult::Err(err),
                },
                b':' => {
                    let name = mem::take(&mut self.value);
                    self.name = Some(name);

                    ParsingResult::Pending
                }
                b' ' | b'\t' => ParsingResult::Pending, // Whitespace is ignored
                x => {
                    self.value
                        .push(char::from_u32(x.into()).unwrap_or(char::REPLACEMENT_CHARACTER));

                    ParsingResult::Pending
                }
            }
        }

        fn finish(&mut self) -> Result<Vec<DataValue>, ParseError> {
            self.complete_value()?;
            let result = self.completed_values.clone();
            self.reset();
            Ok(result)
        }

        fn complete_value(&mut self) -> Result<(), ParseError> {
            let value = self.value.parse().map_err(|_x| ParseError::InvalidFormat)?;
            let data_value = match self.name.take() {
                None => DataValue {
                    name: self.completed_values.len().to_string(),
                    value,
                },
                Some(name) => DataValue { name, value },
            };
            self.completed_values.push(data_value);
            self.value.clear();
            Ok(())
        }

        fn reset(&mut self) {
            self.name = None;
            self.value = String::new();
            self.completed_values.clear();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn simple_test() {
            parser_test(
                "X:0,Y:0",
                vec![
                    DataValue {
                        name: "X".to_string(),
                        value: 0.0,
                    },
                    DataValue {
                        name: "Y".to_string(),
                        value: 0.0,
                    },
                ],
            )
        }

        #[test]
        fn should_parse_data_without_names() {
            parser_test(
                "0,0",
                vec![
                    DataValue {
                        name: "0".to_string(),
                        value: 0.0,
                    },
                    DataValue {
                        name: "1".to_string(),
                        value: 0.0,
                    },
                ],
            )
        }

        #[test]
        fn multi_line_test() {
            let data = b"0,0\n1,1";
            let mut parser = Parser::new();

            for byte in &data[..3] {
                assert_eq!(parser.parse(*byte), ParsingResult::Pending);
            }

            assert_eq!(
                parser.parse(data[3]),
                ParsingResult::Ok(vec![
                    DataValue {
                        name: "0".to_string(),
                        value: 0.0,
                    },
                    DataValue {
                        name: "1".to_string(),
                        value: 0.0,
                    },
                ],)
            );

            for byte in &data[4..] {
                assert_eq!(parser.parse(*byte), ParsingResult::Pending);
            }

            assert_eq!(
                parser.finish(),
                Result::Ok(vec![
                    DataValue {
                        name: "0".to_string(),
                        value: 1.0,
                    },
                    DataValue {
                        name: "1".to_string(),
                        value: 1.0,
                    },
                ],)
            )
        }

        fn parser_test(data: &str, expected_values: Vec<DataValue>) {
            let mut parser = Parser::new();

            for byte in data.bytes() {
                assert_eq!(parser.parse(byte), ParsingResult::Pending);
            }

            assert_eq!(parser.finish(), Result::Ok(expected_values))
        }
    }
}
