use super::*;
use config::saysdf_keys::{parse, Key};
use termwiz::terminal::Terminal;

fn pair() -> (TermWizTerminal, TermWizTerminalPane) {
    let render_pipe = Pipe::new().unwrap();
    let (input_tx, input_rx) = channel();
    let term = TermWizTerminal {
        render_tx: TermWizTerminalRenderTty {
            render_tx: BufWriter::new(render_pipe.write),
            screen_size: ScreenSize {
                cols: 80,
                rows: 24,
                xpixel: 0,
                ypixel: 0,
            },
        },
        input_rx,
        renderer: termwiz_funcs::new_wezterm_terminfo_renderer(),
        grab_mouse: true,
        last_key_phys: None,
    };
    let pane =
        TermWizTerminalPane::new(0, TerminalSize::default(), input_tx, render_pipe.read, None);
    (term, pane)
}

fn poll_key(term: &mut TermWizTerminal) -> (KeyCode, Option<Key>) {
    match term.poll_input(Some(Duration::ZERO)).unwrap() {
        Some(InputEvent::Key(event)) => (event.key, term.last_key_phys()),
        other => panic!("expected a key event, got {:?}", other),
    }
}

#[test]
fn two_queued_keys_keep_their_physical_keys_in_order() {
    let (mut term, pane) = pair();
    let y = parse("y").unwrap();
    let n = parse("Shift+n").unwrap();
    pane.saysdf_key_down_phys(KeyCode::Char('н'), Modifiers::NONE, Some(y)) // i18n-allow-cyrillic
        .unwrap();
    pane.saysdf_key_down_phys(KeyCode::Char('Т'), Modifiers::SHIFT, Some(n)) // i18n-allow-cyrillic
        .unwrap();

    assert_eq!(poll_key(&mut term), (KeyCode::Char('н'), Some(y))); // i18n-allow-cyrillic
    assert_eq!(poll_key(&mut term), (KeyCode::Char('Т'), Some(n))); // i18n-allow-cyrillic
}

#[test]
fn key_without_physical_key_reports_none() {
    let (mut term, pane) = pair();
    let y = parse("y").unwrap();
    pane.saysdf_key_down_phys(KeyCode::Char('y'), Modifiers::NONE, Some(y))
        .unwrap();
    pane.key_down(KeyCode::Char('x'), Modifiers::NONE).unwrap();
    pane.send_paste("paste").unwrap();
    pane.saysdf_key_down_phys(KeyCode::Char('n'), Modifiers::NONE, None)
        .unwrap();

    assert_eq!(poll_key(&mut term), (KeyCode::Char('y'), Some(y)));
    assert_eq!(poll_key(&mut term), (KeyCode::Char('x'), None));
    assert!(matches!(
        term.poll_input(Some(Duration::ZERO)).unwrap(),
        Some(InputEvent::Paste(_))
    ));
    assert_eq!(term.last_key_phys(), None);
    assert_eq!(poll_key(&mut term), (KeyCode::Char('n'), None));
}
