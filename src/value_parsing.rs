use std::{
    io::{BufRead, BufReader},
    str::FromStr,
    sync::mpsc::{SendError, Sender},
    thread,
    time::Instant,
};

pub struct DataValue {
    pub name: String,
    pub value: f64,
    pub timestamp: Instant,
}

use serialport::SerialPort;
use tracing::{info, warn};

pub struct SerialSource {
    port: Box<dyn SerialPort>,
}

impl SerialSource {
    pub fn start(port: Box<dyn SerialPort>, datasender: std::sync::mpsc::Sender<DataValue>) {
        info!("Start reading from {:?}", port.name());
        thread::spawn(move || process_serial_data(port, datasender));
    }
}

fn process_serial_data(
    port: Box<dyn SerialPort>,
    mut datasender: std::sync::mpsc::Sender<DataValue>,
) {
    let name = port.name();
    info!("Start reading from {:?}", &name);
    let mut bufreader = BufReader::new(port);
    let mut line = String::new();
    loop {
        line.clear();
        let result = bufreader.read_line(&mut line);
        let result = match result {
            Ok(0) => Ok(()),
            Ok(x) => {
                bufreader.consume(x);
                parse_data(&line, &mut datasender)
            }
            Err(err) => {
                warn!("Error reading from buffer: {}", err);
                Err(ParseError::ChannelClosed)
            }
        };
        match result {
            Ok(_) | Err(ParseError::InvalidFormat) => {}
            Err(ParseError::ChannelClosed) => break,
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
    let variables = data.split_terminator(',');
    let time_stamp = Instant::now();
    for variable in variables {
        let elements: Vec<_> = variable.split_terminator(':').collect();
        let [name, value, ..] = elements[..] else { return Err(ParseError::InvalidFormat) };
        let value = f64::from_str(value.trim()).map_err(|x| {
            warn!("Error parsing {:?}: {}", value.trim(), x);
            x
        });
        if let Ok(value) = value {
            let data = DataValue {
                name: name.to_string(),
                value,
                timestamp: time_stamp,
            };
            sender.send(data)?;
        }
    }
    Ok(())
}
