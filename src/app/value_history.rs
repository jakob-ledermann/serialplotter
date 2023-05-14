use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
};

use crossbeam::channel::{Receiver, TryRecvError};
use egui::{
    plot::{Legend, Line, Plot, PlotPoints},
    Ui,
};
use tracing::info;

use crate::value_parsing::DataValue;

pub struct ValueHistory {
    buffers: HashMap<String, VecDeque<f64>>,
    cap: usize,
}

impl ValueHistory {
    pub fn try_receive(&mut self, rx: &mut Receiver<DataValue>) -> bool {
        #[cfg(feature = "profiling")]
        puffin::profile_scope!("receive data");

        match rx.try_recv() {
            Err(TryRecvError::Disconnected) => false,
            Err(TryRecvError::Empty) => false, // Great we are faster at consuming than producing (Blocking is not available as this thread must render the ui)
            Ok(DataValue { value, name, .. }) => {
                self.store_value(value, Cow::Owned(name));
                true
            }
        }
    }

    pub fn render_plot(&self, ui: &mut Ui) {
        #[cfg(feature = "profiling")]
        puffin::profile_scope!("plot_rendering");

        let lines = self.buffers.iter().map(|(name, buffer)| {
            let series: Vec<f64> = buffer.iter().copied().collect();
            info!("Dataseries {} with {} points", &name, series.len());
            Line::new(PlotPoints::from_ys_f64(&series)).name(name)
        });
        Plot::new("my_plot")
            .view_aspect(2.0)
            .auto_bounds_x()
            .auto_bounds_y()
            .legend(Legend::default())
            .show(ui, |plot_ui| {
                lines.for_each(|line| plot_ui.line(line));
            });
    }

    pub fn with_capacity(capacity: usize) -> Self {
        ValueHistory {
            buffers: HashMap::new(),
            cap: capacity,
        }
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.cap = capacity;
        for (_name, buffer) in self.buffers.iter_mut() {
            while buffer.len() >= self.cap {
                buffer.pop_front();
            }
        }
    }

    pub fn update(
        &mut self,
        receiver: &mut Receiver<DataValue>,
        displayed_values: usize,
        max_fetch_count: usize,
    ) {
        #[cfg(feature = "profiling")]
        puffin::profile_scope!("update serial values");

        self.set_capacity(displayed_values);
        let mut count = max_fetch_count;
        while self.try_receive(receiver) && count > 0 {
            count -= 1;
        }

        let count = max_fetch_count - count;
        self.store_value(count as f64, Cow::Borrowed("fetch_count"));

        self.store_value(receiver.len() as f64, Cow::Borrowed("pending_messages"));
    }

    fn store_value(&mut self, value: f64, key: Cow<'_, str>) {
        let buffer = self
            .buffers
            .entry(key.into_owned())
            .or_insert_with(|| VecDeque::with_capacity(self.cap));

        buffer.push_back(value);
        if buffer.len() >= self.cap {
            buffer.pop_front();
        }
    }
}
