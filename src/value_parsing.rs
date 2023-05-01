use std::{
    io::{self, BufRead, BufReader},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, SendError, Sender};

#[derive(Debug, PartialEq)]
pub struct DataValue {
    pub name: String,
    pub value: f64,
}

use serialport::SerialPort;
use tracing::{debug, info, warn};

pub struct SerialSource {
    port: Box<dyn SerialPort>,
}

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
        thread::Builder::new()
            .name(format!("Read serial {}", port.name().unwrap()))
            .spawn(move || process_serial_data(port, datasender, command_receiver));
    }
}

fn process_serial_data(
    mut port: Box<dyn SerialPort>,
    mut datasender: Sender<DataValue>,
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
    let mut offset = 0;
    let mut minimum_message_size = buffer.len();
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
        datasender.send(DataValue {
            name: "reading".to_string(),
            value: 1000f64,
        });
        let result = port.read(&mut buffer[offset..offset + 1]);
        {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("processing received data");
            let result = match result {
                Ok(amount) => {
                    let mut usable = &buffer[..(amount + offset)];
                    let mut total_consumed = 0;
                    offset += amount;
                    while let Ok(consumed) = usable.read_line(&mut line) {
                        if consumed == 0 {
                            break;
                        }
                        match parse_data(&line, &mut datasender) {
                            Err(ParseError::ChannelClosed) => break 'read_loop,
                            Err(ParseError::InvalidFormat) => {
                                total_consumed += consumed;
                                minimum_message_size *= 2;
                            }
                            _ => {
                                total_consumed += consumed;
                                if minimum_message_size > consumed {
                                    minimum_message_size = consumed.max(1);
                                }
                            }
                        }

                        line.clear();
                    }
                    if total_consumed > 0 {
                        buffer.copy_within(total_consumed.., 0);
                        offset -= total_consumed;
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

enum ParseError {
    ChannelClosed,
    InvalidFormat,
}

impl From<SendError<DataValue>> for ParseError {
    fn from(_value: SendError<DataValue>) -> Self {
        Self::ChannelClosed
    }
}

fn parse_data(data: &str, sender: &mut Sender<DataValue>) -> Result<(), ParseError> {
    debug!("Parsing {}", data);
    let variables = data.split_terminator(',');
    let mut result = Ok(());
    for variable in variables {
        let elements: Vec<_> = variable.split_terminator(':').collect();
        let (name, value) = match elements[..] {
            [value] => ("", value.trim()),
            [name, value] => (name.trim(), value.trim()),
            _ => return Err(ParseError::InvalidFormat),
        };
        let value = value.parse().map_err(|x| {
            warn!("Error parsing {:?}: {}", value, x);
            x
        });
        if let Ok(value) = value {
            let data = DataValue {
                name: name.to_string(),
                value,
            };
            sender.send(data)?;
        } else {
            result = Err(ParseError::InvalidFormat);
        }
    }
    info!(target: "pending_data", count=sender.len(), "Sender sees {} pending messages", sender.len());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_test() {
        let sample = "X:0,Y:0";
        let (mut tx, rx) = crossbeam::channel::unbounded();
        parse_data(sample, &mut tx);

        assert_eq!(
            rx.recv(),
            Ok(DataValue {
                name: String::from("X"),
                value: 0.0
            })
        );
        assert_eq!(
            rx.recv(),
            Ok(DataValue {
                name: String::from("Y"),
                value: 0.0
            })
        );
    }
}
