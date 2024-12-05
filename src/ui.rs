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

pub fn run<S>(stat: Arc<S>, tx: Sender<GeneratorEvent>) -> Result<(), Box<dyn Error + Send + Sync>>
where
    S: CommonStat + TxStat + SocketStat + 'static,
{
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = Ui::new(stat, tx);
    let rc = app.run(&mut terminal, Duration::from_millis(25));

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    rc
}

pub struct Ui<S> {
    tx: Sender<GeneratorEvent>,
    stat: Arc<S>,
    head: StatusWidget,
    tx_stat: TxStatWidget<S>,
    sock: SockStatWidget<S>,
    input: InputWidget,
    keymap: KeymapWidget,
}

impl<S> Ui<S>
where
    S: CommonStat + TxStat + SocketStat + 'static,
{
    pub fn new(stat: Arc<S>, tx: Sender<GeneratorEvent>) -> Self {
        let head = StatusWidget::new();
        let tx_stat = TxStatWidget::new();
        let sock = SockStatWidget::new();
        let input = InputWidget::new();
        let keymap = KeymapWidget::new();

        Self {
            tx,
            stat,
            head,
            keymap,
            tx_stat,
            input,
            sock,
        }
    }

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
        self.sock.draw(frame, sock);
    }

    pub fn on_tick(&mut self) {
        self.tx_stat.update(&self.stat);
        self.sock.update(&self.stat);
    }
}

struct MetricWidget<S> {
    name: Cow<'static, str>,
    metric: Box<dyn Metric<S>>,
    name_style: Style,
    metric_style: Style,
}

impl<S> MetricWidget<S> {
    pub fn new<T>(name: T, metric: Box<dyn Metric<S>>) -> Self
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

    pub fn update(&mut self, stat: &S) {
        self.metric.update(stat);
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

struct MetricListWidget<S> {
    name: Cow<'static, str>,
    widgets: Vec<MetricWidget<S>>,
}

impl<S> MetricListWidget<S> {
    pub fn new<T>(name: T, widgets: Vec<MetricWidget<S>>) -> Self
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

    pub fn update(&mut self, stat: &S) {
        for widget in &mut self.widgets {
            widget.update(stat);
        }
    }
}

struct TxStatWidget<S> {
    widget: MetricListWidget<S>,
}

impl<S> TxStatWidget<S>
where
    S: CommonStat + TxStat + 'static,
{
    pub fn new() -> Self {
        let widgets = vec![
            MetricWidget::new("RPS expected ", Box::new(Gauge::new(|s: &S| s.generator()))),
            MetricWidget::new(
                "RPS current  ",
                Box::new(Meter::new(Box::new(|s: &S| s.num_requests()))),
            ),
            MetricWidget::new("Requests sent", Box::new(Gauge::new(|s: &S| s.num_requests()))),
            MetricWidget::new("Bitrate TX   ", Box::new(Throughput::new(|s: &S| s.bytes_tx()))),
        ];

        let widget = MetricListWidget::new("Requests", widgets);

        Self { widget }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.widget.draw(frame, area);
    }

    pub fn update(&mut self, stat: &S) {
        self.widget.update(stat);
    }
}

struct SockStatWidget<S> {
    widget: MetricListWidget<S>,
}

impl<S> SockStatWidget<S>
where
    S: SocketStat + 'static,
{
    pub fn new() -> Self {
        let widgets = vec![
            MetricWidget::new("Created", Box::new(Gauge::new(|s: &S| s.num_sock_created()))),
            MetricWidget::new("Rate   ", Box::new(Meter::new(Box::new(|s: &S| s.num_sock_created())))),
            MetricWidget::new("Errors ", Box::new(Gauge::new(|s: &S| s.num_sock_errors())))
                .with_metric_style(Style::default().fg(Color::Red)),
        ];

        let widget = MetricListWidget::new("Sockets", widgets);

        Self { widget }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.widget.draw(frame, area);
    }

    pub fn update(&mut self, stat: &S) {
        self.widget.update(stat);
    }
}
