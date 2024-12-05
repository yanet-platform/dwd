use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Manual,
    Running(bool),
}

pub struct StatusWidget {
    version: &'static str,
    mode: Mode,
}

impl StatusWidget {
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            mode: Mode::Running(true),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                "DWD",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        frame.render_widget(block, area);

        let [area] = Layout::vertical([Constraint::Percentage(100)])
            .horizontal_margin(2)
            .vertical_margin(1)
            .areas(area);

        let mut text = vec![Line::from(vec![Span::raw("Version: "), Span::raw(self.version)])];
        let span = match self.mode {
            Mode::Manual => Span::styled("Manual", Style::default().fg(Color::Yellow)),
            Mode::Running(true) => Span::styled("Running", Style::default().fg(Color::Green)),
            Mode::Running(false) => Span::styled("Suspended", Style::default().fg(Color::Red)),
        };
        text.push(Line::from(vec![Span::raw("Mode: "), span]));

        let widget = Paragraph::new(text).wrap(Wrap { trim: true });

        frame.render_widget(widget, area);
    }
}
