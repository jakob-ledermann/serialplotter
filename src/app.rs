use std::collections::{hash_map::Entry, HashMap, VecDeque};

use egui::plot::{Legend, Line, Plot, PlotPoints};
use egui::{InnerResponse, Ui};

use crossbeam::channel::{Receiver, Sender, TryRecvError};
use serialport::available_ports;
use tracing::info;

use crate::value_parsing::Commands;
use crate::{
    frame_history::{self, FrameHistory},
    value_parsing::{DataValue, SerialSource},
};

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct TemplateApp {
    // this how you opt-out of serialization of a member
    displayed_values: usize,
    max_fetch_count: usize,

    serial_port_name: Option<String>,
    baud_rate: u32,

    #[serde(skip)]
    show_log: bool,

    #[serde(skip)]
    value_history: ValueHistory,

    #[serde(skip)]
    receiver: Receiver<DataValue>,

    #[serde(skip)]
    sender: Sender<DataValue>,

    #[serde(skip)]
    open_port: Option<(String, u32)>,

    #[serde(skip)]
    fps_history: frame_history::FrameHistory,
    #[serde(skip)]
    command: (Sender<Commands>, Receiver<Commands>),
}

impl Default for TemplateApp {
    fn default() -> Self {
        let (tx, rx) = crossbeam::channel::bounded(10000);
        let (command_tx, command_rx) = crossbeam::channel::bounded(10);
        Self {
            // Example stuff:
            displayed_values: 1000,
            max_fetch_count: 100,
            serial_port_name: None,
            baud_rate: 9600,
            value_history: ValueHistory::with_capacity(1000),
            receiver: rx,
            sender: tx,
            open_port: None,
            show_log: true,
            fps_history: FrameHistory::default(),
            command: (command_tx, command_rx),
        }
    }
}

impl TemplateApp {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }

        Default::default()
    }
}

impl eframe::App for TemplateApp {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn on_close_event(&mut self) -> bool {
        let Self { command, .. } = self;
        command.0.send(Commands::Stop);
        true
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let Self {
            serial_port_name,
            baud_rate,
            value_history,
            receiver,
            sender,
            open_port,
            max_fetch_count,
            displayed_values,
            show_log,
            fps_history,
            command,
            ..
        } = self;

        fps_history.on_new_frame(ctx.input(|x| x.time), None);

        #[cfg(feature = "profiling")]
        {
            puffin::profile_function!();
            puffin::GlobalProfiler::lock().new_frame(); // call once per frame!
            puffin_egui::profiler_window(ctx);
        }

        value_history.update(receiver, *displayed_values, *max_fetch_count);

        // Examples of how to create different panels and windows.
        // Pick whichever suits you.
        // Tip: a good default choice is to just keep the `CentralPanel`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        #[cfg(not(target_arch = "wasm32"))] // no File->Quit on web pages!
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("top_panel");

            // The top panel is often a good place for a menu bar:
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        _frame.close();
                    }
                });
            });
        });

        egui::SidePanel::left("side_panel").show(ctx, |ui| {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("side panel");

            ui.heading("Side Panel");

            let mut scaled_value = (*displayed_values).clamp(0usize, 10000usize) as f64 / 1000.0;
            ui.add(egui::Slider::new(&mut scaled_value, 0.0..=10.0).text("displayed values"));

            *displayed_values = (scaled_value * 1000.0).round().clamp(100.0, 10000.0) as usize;

            let mut scaled_value = (*max_fetch_count).clamp(1, 10000) as f64 / 1000.0;
            ui.add(egui::Slider::new(&mut scaled_value, 0.0..=10.0).text("fetch count"));

            *max_fetch_count = (scaled_value * 1000.0).round().clamp(10f64, 10000f64) as usize;

            ui.checkbox(show_log, "Show tracing log");

            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.label("Serialport configuration");
                create_serial_port_selection(ui, serial_port_name);
                create_baud_rate_selection(ui, baud_rate);

                match (&open_port, serial_port_name) {
                    (None, Some(serial_port_name)) => {
                        if ui.button("open").clicked() {
                            *open_port = open_serial_port(
                                serial_port_name.clone(),
                                baud_rate,
                                sender,
                                command.1.clone(),
                            );
                        }
                    }
                    (Some(_), _) => {
                        if ui.button("close").clicked() {
                            close_serial_port(command);
                            *open_port = None
                        }
                    }
                    (None, None) => {}
                }
            });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("powered by ");
                    ui.hyperlink_to("egui", "https://github.com/emilk/egui");
                    ui.label(" and ");
                    ui.hyperlink_to(
                        "eframe",
                        "https://github.com/emilk/egui/tree/master/crates/eframe",
                    );
                    ui.label(".");
                });

                ui.label(format!("FPS: {}", fps_history.fps()));
                //fps_history.ui(ui);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // The central panel the region left after adding TopPanel's and SidePanel's
            value_history.render_plot(ui);

            egui::warn_if_debug_build(ui);
        });

        if *show_log {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("Display Log");

            egui::TopBottomPanel::bottom("log").show(ctx, |ui| {
                let widget = tracing_egui::Widget {
                    filter: true,
                    ..Default::default()
                };
                ui.add(widget);
            });
        }

        if false {
            egui::Window::new("Window").show(ctx, |ui| {
                ui.label("Windows can be moved by dragging them.");
                ui.label("They are automatically sized based on contents.");
                ui.label("You can turn on resizing and scrolling if you like.");
                ui.label("You would normally choose either panels OR windows.");
            });
        }

        ctx.request_repaint();
        //ctx.request_repaint_after(Duration::from_secs_f64(0.05));
    }
}

fn close_serial_port(command: &mut (Sender<Commands>, Receiver<Commands>)) {
    command.0.send(Commands::Stop);
}

fn open_serial_port(
    serial_port_name: String,
    baud_rate: &u32,
    sender: &mut Sender<DataValue>,
    command: Receiver<Commands>,
) -> Option<(String, u32)> {
    let port = match serialport::new(
        std::borrow::Cow::Owned(serial_port_name.clone()),
        *baud_rate,
    )
    .open()
    {
        Ok(port) => Some(port),
        Err(err) => {
            tracing::error!("Failed to open port: {}", err);
            None
        }
    };

    port.map(|x| SerialSource::start(x, sender.clone(), command))
        .map(|_| (serial_port_name.clone(), *baud_rate))
}

fn create_serial_port_selection(
    ui: &mut Ui,
    serial_port_name: &mut Option<String>,
) -> InnerResponse<Option<()>> {
    let ports = available_ports().unwrap_or(Vec::new());
    egui::ComboBox::from_label("Serial port")
        .selected_text(format!("{:?}", serial_port_name))
        .show_ui(ui, |ui| {
            for port in ports {
                ui.selectable_value(
                    serial_port_name,
                    Some(port.port_name.clone()),
                    port.port_name,
                );
            }
        })
}

fn create_baud_rate_selection(ui: &mut Ui, baud_rate: &mut u32) -> InnerResponse<Option<()>> {
    egui::ComboBox::from_label("Baud rate")
        .selected_text(format!("{:?}", baud_rate))
        .show_ui(ui, |ui| {
            ui.selectable_value(baud_rate, 9600, "9600");
            ui.selectable_value(baud_rate, 115200, "115200");
            // TODO weitere Baudraten anbieten
        })
}

struct ValueHistory {
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
            Ok(value) => {
                self.store_value(value.value, value.name.clone());
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

    fn with_capacity(capacity: usize) -> Self {
        ValueHistory {
            buffers: HashMap::new(),
            cap: capacity,
        }
    }

    fn set_capacity(&mut self, capacity: usize) {
        self.cap = capacity;
        for (_name, buffer) in self.buffers.iter_mut() {
            while buffer.len() >= self.cap {
                buffer.pop_front();
            }
        }
    }

    fn update(
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
        self.store_value(count as f64, "fetch_count".to_string());

        self.store_value(receiver.len() as f64, "pending_messages".to_string());
    }

    fn store_value(&mut self, value: f64, key: String) {
        match self.buffers.entry(key) {
            Entry::Occupied(mut entry) => {
                let buffer = entry.get_mut();
                buffer.push_back(value);
                if buffer.len() >= self.cap {
                    buffer.pop_front();
                }
            }
            Entry::Vacant(entry) => {
                let mut buffer = VecDeque::with_capacity(self.cap);
                buffer.push_back(value);

                entry.insert(buffer);
            }
        }
    }
}
