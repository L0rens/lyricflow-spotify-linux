use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Gauge, Paragraph},
};

pub fn draw_progress(
    frame: &mut Frame,
    area: Rect,
    position: u64,
    duration: u64,
    active: Color,
    muted: Color,
) {
    let elapsed = crate::timestamp(position);
    let total = crate::timestamp(duration);
    let left = elapsed.len() as u16 + 1;
    let right = total.len() as u16 + 1;
    if area.width < left + right + 1 {
        frame.render_widget(
            Paragraph::new(format!("{elapsed} / {total}")).style(Style::default().fg(muted)),
            area,
        );
        return;
    }
    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left),
            Constraint::Min(1),
            Constraint::Length(right),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(elapsed).style(Style::default().fg(muted)),
        parts[0],
    );
    let ratio = if duration == 0 {
        0.0
    } else {
        (position as f64 / duration as f64).clamp(0.0, 1.0)
    };
    frame.render_widget(
        Gauge::default()
            .ratio(ratio)
            .label("")
            .gauge_style(Style::default().fg(active)),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new(total)
            .alignment(Alignment::Right)
            .style(Style::default().fg(muted)),
        parts[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn hours_and_elapsed_are_separate_from_the_gauge_on_narrow_screens() {
        for width in [20, 30, 40, 80] {
            let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
            terminal
                .draw(|frame| {
                    draw_progress(
                        frame,
                        frame.area(),
                        3_599_000,
                        4_363_000,
                        Color::Green,
                        Color::Gray,
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.starts_with("59:59"));
            assert!(text.ends_with("1:12:43"));
            for x in 0..5 {
                assert_eq!(buffer[(x, 0)].fg, Color::Gray);
            }
            for x in width - 7..width {
                assert_eq!(buffer[(x, 0)].fg, Color::Gray);
            }
        }
    }
}
