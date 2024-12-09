use core::{iter::repeat, ops::Deref, time::Duration};
use std::{borrow::Cow, error::Error, sync::Arc, time::Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    prelude::{Backend, CrosstermBackend},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Block,
    Frame, Terminal,
};
use tokio::sync::mpsc::Sender;
use widget::{
    input::InputWidget,
    status::{Mode, StatusWidget},
};

use self::{
    metric::{Gauge, Meter, Metric, Throughput},
    widget::keymap::KeymapWidget,
};
use crate::{
    stat::{CommonStat, SocketStat, TxStat},
    GeneratorEvent,
};

mod metric;
mod widget;

pub fn run(mut app: Ui) -> Result<(), Box<dyn Error + Send + Sync>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let rc = app.run(&mut terminal, Duration::from_millis(25));

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    rc
}

pub struct Ui {
    tx: Sender<GeneratorEvent>,
    head: StatusWidget,
    tx_stat: TxStatWidget,
    sock: Option<SockStatWidget>,
    input: InputWidget,
    keymap: KeymapWidget,
}

impl Ui {
    pub fn new<S>(stat: Arc<S>, tx: Sender<GeneratorEvent>) -> Self
    where
        S: CommonStat + TxStat + Send + Sync + 'static,
    {
        let head = StatusWidget::new();
        let tx_stat = TxStatWidget::new(stat);
        let input = InputWidget::new();
        let keymap = KeymapWidget::new();

        Self {
            tx,
            head,
            keymap,
            tx_stat,
            input,
            sock: None,
        }
    }

    pub fn with_sock<S>(mut self, stat: Arc<S>) -> Self
    where
        S: SocketStat + Send + Sync + 'static,
    {
        self.sock = Some(SockStatWidget::new(stat));
        self
    }
}

impl Ui {
    pub fn run<B>(&mut self, terminal: &mut Terminal<B>, fps: Duration) -> Result<(), Box<dyn Error + Send + Sync>>
    where
        B: Backend,
    {
        let mut prev_ts = Instant::now();

        loop {
            terminal.draw(|frame| self.draw(frame))?;

            let timeout = fps.saturating_sub(prev_ts.elapsed());
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') => {
                                return Ok(());
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                return Ok(());
                            }
                            KeyCode::Char('s') => {
                                let (ev, is_running) = match self.head.mode() {
                                    Mode::Manual => (GeneratorEvent::Resume, true),
                                    Mode::Running(is_running) => {
                                        let ev = if is_running {
                                            GeneratorEvent::Suspend
                                        } else {
                                            GeneratorEvent::Resume
                                        };
                                        (ev, !is_running)
                                    }
                                };

                                self.head.set_mode(Mode::Running(is_running));
                                _ = self.tx.try_send(ev);
                            }
                            KeyCode::Char(c) => {
                                if c.is_ascii_digit() {
                                    self.input.on_char(c);
                                }
                            }
                            KeyCode::Backspace => {
                                self.input.pop_char();
                            }
                            KeyCode::Enter => {
                                let value = self.input.take();
                                if let Ok(value) = value.parse::<u64>() {
                                    self.head.set_mode(Mode::Manual);
                                    _ = self.tx.try_send(GeneratorEvent::Set(value));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if prev_ts.elapsed() >= fps {
                self.on_tick();
                prev_ts = Instant::now();
            }
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let [head, input, stat, keymap] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(20),
            Constraint::Length(2 + self.keymap.rows()),
        ])
        .areas(frame.area());

        self.head.draw(frame, head);
        self.draw_stats(frame, stat);
        self.input.draw(frame, input);
        self.keymap.draw(frame, keymap);
    }

    fn draw_stats(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                "Stats",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        frame.render_widget(block, area);

        let [tx_stat, sock, _] = Layout::vertical([Constraint::Length(6), Constraint::Length(5), Constraint::Min(1)])
            .horizontal_margin(2)
            .vertical_margin(1)
            .areas(area);

        self.tx_stat.draw(frame, tx_stat);
        if let Some(s) = &mut self.sock {
            s.draw(frame, sock);
        }
    }

    pub fn on_tick(&mut self) {
        self.tx_stat.update();
        if let Some(s) = &mut self.sock {
            s.update();
        }
    }
}

struct MetricWidget {
    name: Cow<'static, str>,
    metric: Box<dyn Metric + Send>,
    name_style: Style,
    metric_style: Style,
}

impl MetricWidget {
    pub fn new<T>(name: T, metric: Box<dyn Metric + Send>) -> Self
    where
        T: Into<Cow<'static, str>>,
    {
        let name = name.into();
        let name_style = Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD);
        let metric_style = Style::default();

        Self {
            name,
            metric,
            name_style,
            metric_style,
        }
    }

    pub fn with_metric_style(mut self, style: Style) -> Self {
        self.metric_style = style;
        self
    }

    pub fn update(&mut self) {
        self.metric.update();
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let text = Line::from(vec![
            Span::styled(self.name.deref(), self.name_style),
            Span::raw(": "),
            Span::styled(format!("{}", self.metric), self.metric_style),
        ]);
        frame.render_widget(text, area);
    }
}

struct MetricListWidget {
    name: Cow<'static, str>,
    widgets: Vec<MetricWidget>,
}

impl MetricListWidget {
    pub fn new<T>(name: T, widgets: Vec<MetricWidget>) -> Self
    where
        T: Into<Cow<'static, str>>,
    {
        Self { name: name.into(), widgets }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                self.name.deref(),
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        frame.render_widget(block, area);

        let areas = Layout::vertical(repeat(Constraint::Length(1)).take(self.widgets.len()))
            .margin(1)
            .horizontal_margin(2)
            .split(area);

        for idx in 0..self.widgets.len() {
            self.widgets[idx].draw(frame, areas[idx]);
        }
    }

    pub fn update(&mut self) {
        for widget in &mut self.widgets {
            widget.update();
        }
    }
}

struct TxStatWidget {
    widget: MetricListWidget,
}

impl TxStatWidget {
    pub fn new<S>(stat: Arc<S>) -> Self
    where
        S: CommonStat + TxStat + Send + Sync + 'static,
    {
        let widgets = vec![
            MetricWidget::new("RPS expected ", Box::new(Gauge::new(|s| s.generator(), stat.clone()))),
            MetricWidget::new(
                "RPS current  ",
                Box::new(Meter::new(|s| s.num_requests(), stat.clone())),
            ),
            MetricWidget::new(
                "Requests sent",
                Box::new(Gauge::new(|s| s.num_requests(), stat.clone())),
            ),
            MetricWidget::new(
                "Bitrate TX   ",
                Box::new(Throughput::new(|s| s.bytes_tx(), stat.clone())),
            ),
        ];

        let widget = MetricListWidget::new("Requests", widgets);

        Self { widget }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.widget.draw(frame, area);
    }

    pub fn update(&mut self) {
        self.widget.update();
    }
}

struct SockStatWidget {
    widget: MetricListWidget,
}

impl SockStatWidget {
    pub fn new<S>(stat: Arc<S>) -> Self
    where
        S: SocketStat + Send + Sync + 'static,
    {
        let widgets = vec![
            MetricWidget::new("Created", Box::new(Gauge::new(|s| s.num_sock_created(), stat.clone()))),
            MetricWidget::new("Rate   ", Box::new(Meter::new(|s| s.num_sock_created(), stat.clone()))),
            MetricWidget::new("Errors ", Box::new(Gauge::new(|s| s.num_sock_errors(), stat.clone())))
                .with_metric_style(Style::default().fg(Color::Red)),
        ];

        let widget = MetricListWidget::new("Sockets", widgets);

        Self { widget }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.widget.draw(frame, area);
    }

    pub fn update(&mut self) {
        self.widget.update();
    }
}
