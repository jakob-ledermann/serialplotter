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
use tracing::warn;

pub struct SerialSource {
    port: Box<dyn SerialPort>,
}

impl SerialSource {
    pub fn start(port: Box<dyn SerialPort>, datasender: std::sync::mpsc::Sender<DataValue>) {
        thread::spawn(|| process_serial_data(port, datasender));
    }
}

fn process_serial_data(
    port: Box<dyn SerialPort>,
    mut datasender: std::sync::mpsc::Sender<DataValue>,
) {
    let port_name = port.name();
    let mut bufreader = BufReader::new(port);
    bufreader.lines().map(|line| {
        line.map_err(|err| {
            warn!("Error reading from buffer: {}", err);
            ParseError::ChannelClosed
        })
        .and_then(|line| parse_data(&line, &mut datasender))
    });
}

enum ParseError {
    ChannelClosed,
    InvalidFormat,
}

impl From<SendError<DataValue>> for ParseError {
    fn from(value: SendError<DataValue>) -> Self {
        Self::ChannelClosed
    }
}

fn parse_data(data: &str, sender: &mut Sender<DataValue>) -> Result<(), ParseError> {
    let variables = data.split_terminator(',');
    let timeStamp = Instant::now();
    for variable in variables {
        let elements = variable.split_terminator(':').collect::<Vec<&str>>();
        let [name, value, ..] = elements[..] else { return Err(ParseError::InvalidFormat)};
        let value = f64::from_str(value);
        if let Ok(value) = value {
            let data = DataValue {
                name: name.to_string(),
                value,
                timestamp: timeStamp,
            };
            sender.send(data)?;
        }
    }
    Ok(())
}
