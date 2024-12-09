use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Block,
    Frame,
};

pub struct InputWidget {
    value: String,
}

impl InputWidget {
    pub fn new() -> Self {
        Self { value: String::new() }
    }

    pub fn take(&mut self) -> String {
        core::mem::take(&mut self.value)
    }

    pub fn on_char(&mut self, c: char) {
        self.value.push(c);
    }

    pub fn pop_char(&mut self) -> Option<char> {
        self.value.pop()
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                "Input",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        frame.render_widget(block, area);

        let [area] = Layout::vertical([Constraint::Percentage(100)])
            .horizontal_margin(2)
            .vertical_margin(1)
            .areas(area);
        let [input, cursor] =
            Layout::horizontal([Constraint::Length(self.value.len() as u16), Constraint::Length(1)]).areas(area);

        let line = Line::from(self.value.as_str()).style(Style::default().fg(Color::Yellow));
        let line_cursor: Line<'_> = Line::from("█");

        frame.render_widget(line, input);
        frame.render_widget(line_cursor, cursor);
    }
}
