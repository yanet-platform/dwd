use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

#[derive(Debug)]
pub struct KeymapWidget {
    widget: Paragraph<'static>,
    rows: u16,
}

impl KeymapWidget {
    pub fn new() -> Self {
        let keymaps = [
            ("Suspend | Resume", &["s"][..]),
            ("Enter profile manually", &["0-9"]),
            ("Submit manual profile", &["Enter"]),
            ("Exit", &["q", "ctrl+c"][..]),
        ];

        let mut text = Vec::new();
        for (name, keys) in keymaps {
            let mut spans = Vec::new();
            spans.push(Span::raw(name));
            spans.push(Span::raw(": "));

            let mut keys = keys.iter();
            if let Some(key) = keys.next() {
                spans.push(Span::styled(*key, Style::default().fg(Color::Green)));
            }
            for key in keys {
                spans.push(Span::raw(" or "));
                spans.push(Span::styled(*key, Style::default().fg(Color::Green)));
            }

            text.push(Line::from(spans));
        }

        let widget = Paragraph::new(text).wrap(Wrap { trim: true });

        Self { widget, rows: keymaps.len() as u16 }
    }

    #[inline]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                "Key Bindings",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        frame.render_widget(block, area);

        let [area] = Layout::vertical([Constraint::Percentage(100)])
            .horizontal_margin(2)
            .vertical_margin(1)
            .areas(area);
        frame.render_widget(&self.widget, area);
    }
}
