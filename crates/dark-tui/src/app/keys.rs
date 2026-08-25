//! Key and mouse bindings.
//!
//! See the key table in task unit `H1`. A binding either changes local
//! shell state (a pane cycle, a focus change) and returns `None`, or it asks
//! the harness for something and returns `Some(Intent)` for the caller to
//! send.

use dark_contract::{Allow, Intent};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::app::pane::Focus;
use crate::app::state::App;
use crate::app::zone::ZoneId;

/// How many lines `PageUp` and `PageDown` move the transcript.
///
/// A fixed step rather than the pane's height: `App` records the terminal
/// size but a key handler has no pane geometry, and a step a person can
/// predict beats one that changes with the window.
const SCROLL_PAGE: usize = 10;

/// What [`App::handle_global_key`] found.
enum GlobalKey {
    /// Not a global binding. The caller tries the focus-specific bindings
    /// next.
    Unhandled,
    /// A global binding fired. Send this intent to the harness, if any.
    Handled(Option<Intent>),
}

impl App {
    /// Handles one keyboard event.
    ///
    /// Returns `Some(Intent)` when the harness must hear about this key.
    /// Returns `None` when the key only changed local shell state, or when
    /// it did nothing.
    #[must_use]
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Intent> {
        // The kitty keyboard protocol can report a key release. Nothing in
        // this shell binds to a release; acting on both the press and the
        // release would double every action.
        if key.kind == KeyEventKind::Release {
            return None;
        }

        // A pending confirmation blocks the turn, so its answer keys
        // outrank every other binding: nothing else can run until the
        // person answers, and a stray keystroke must not be read as text.
        if self.is_awaiting_confirm()
            && let Some(intent) = self.handle_confirm_key(key)
        {
            return Some(intent);
        }

        if let GlobalKey::Handled(outcome) = self.handle_global_key(key) {
            return outcome;
        }

        if self.focus() == Focus::Command {
            return self.handle_command_bar_key(key);
        }

        self.handle_pane_key(key)
    }

    /// Bindings that fire regardless of which region has focus.
    fn handle_global_key(&mut self, key: KeyEvent) -> GlobalKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::F(1) => {
                self.toggle_help();
                GlobalKey::Handled(None)
            }
            KeyCode::F(2) => {
                self.set_left_pane(crate::app::pane::LeftPane::Map);
                GlobalKey::Handled(None)
            }
            KeyCode::F(3) => {
                self.set_right_pane(crate::app::pane::RightPane::Transcript);
                GlobalKey::Handled(None)
            }
            KeyCode::F(4) => {
                self.set_right_pane(crate::app::pane::RightPane::Diff);
                GlobalKey::Handled(None)
            }
            KeyCode::F(5) => {
                self.set_right_pane(crate::app::pane::RightPane::Explore);
                GlobalKey::Handled(None)
            }
            KeyCode::F(6) => GlobalKey::Handled(Some(Intent::Command("/lexicon".to_owned()))),
            KeyCode::F(7) => GlobalKey::Handled(Some(Intent::Command("/ticket".to_owned()))),
            KeyCode::F(8) => GlobalKey::Handled(Some(Intent::Command("/resolve".to_owned()))),
            KeyCode::F(9) => {
                self.toggle_menu();
                GlobalKey::Handled(None)
            }
            KeyCode::F(10) => {
                self.request_quit();
                GlobalKey::Handled(Some(Intent::Quit))
            }
            KeyCode::Tab => {
                self.set_focus(self.focus().next());
                GlobalKey::Handled(None)
            }
            KeyCode::Left if ctrl => {
                self.cycle_focused_pane(false);
                GlobalKey::Handled(None)
            }
            KeyCode::Right if ctrl => {
                self.cycle_focused_pane(true);
                GlobalKey::Handled(None)
            }
            KeyCode::Char('p' | 'P') if ctrl => {
                self.toggle_menu();
                GlobalKey::Handled(None)
            }
            KeyCode::Char('d' | 'D') if ctrl => {
                let go_dark = !self.header().dark;
                GlobalKey::Handled(Some(Intent::GoDark(go_dark)))
            }
            KeyCode::PageUp => {
                self.scroll_back(SCROLL_PAGE);
                GlobalKey::Handled(None)
            }
            KeyCode::PageDown => {
                self.scroll_forward(SCROLL_PAGE);
                GlobalKey::Handled(None)
            }
            KeyCode::End if ctrl => {
                self.scroll_to_tail();
                GlobalKey::Handled(None)
            }
            KeyCode::Esc => GlobalKey::Handled(self.handle_escape()),
            KeyCode::Char('c' | 'C') if ctrl => GlobalKey::Handled(Some(self.handle_ctrl_c())),
            _ => GlobalKey::Unhandled,
        }
    }

    /// Cycles whichever pane holds focus. `forward` of `true` moves to the
    /// next pane; `false` moves to the previous one. Does nothing while the
    /// command bar has focus.
    fn cycle_focused_pane(&mut self, forward: bool) {
        match self.focus() {
            Focus::Left => {
                let pane = if forward {
                    self.left_pane().next()
                } else {
                    self.left_pane().prev()
                };
                self.set_left_pane(pane);
            }
            Focus::Right => {
                let pane = if forward {
                    self.right_pane().next()
                } else {
                    self.right_pane().prev()
                };
                self.set_right_pane(pane);
            }
            Focus::Command => {}
        }
    }

    /// Bindings while a confirmation is open: `y` allows once, `a` allows
    /// this shape from now on, `n` and `Esc` refuse.
    ///
    /// Returns `None` for every other key, so a person who presses
    /// something else stays in the modal rather than dismissing it by
    /// accident — task unit `A4` treats an unanswered request as a refusal
    /// only when the harness itself decides, never as a side effect of a
    /// keystroke.
    fn handle_confirm_key(&mut self, key: KeyEvent) -> Option<Intent> {
        let allow = match key.code {
            KeyCode::Char('y' | 'Y') => Allow::Once,
            KeyCode::Char('a' | 'A') => Allow::Always,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Allow::Deny,
            _ => return None,
        };
        self.answer_confirm(allow)
    }

    /// `Esc`: cancel a running turn, or close whichever overlay is open, or
    /// clear the command bar. Never more than one of these per press.
    fn handle_escape(&mut self) -> Option<Intent> {
        if self.is_turn_active() {
            // Cancelling the turn closes out whatever "twice during a turn"
            // context `Ctrl+C` was building; the next `Ctrl+C` starts fresh.
            self.disarm_quit();
            return Some(Intent::Cancel);
        }
        if self.is_help_visible() {
            self.toggle_help();
            return None;
        }
        if self.is_menu_visible() {
            self.toggle_menu();
            return None;
        }
        if self.focus() == Focus::Command && !self.command_input().is_empty() {
            self.take_command_input();
        }
        None
    }

    /// `Ctrl+C`: quit at once outside a turn. During a turn, the first press
    /// cancels the turn and arms a confirmation; the second press quits.
    /// See the key table in task unit `H1`: "Ctrl+C quit, twice during a
    /// turn."
    fn handle_ctrl_c(&mut self) -> Intent {
        if self.is_turn_active() {
            if self.is_quit_armed() {
                self.request_quit();
                Intent::Quit
            } else {
                self.arm_quit();
                Intent::Cancel
            }
        } else {
            self.request_quit();
            Intent::Quit
        }
    }

    /// Bindings while the command bar has focus: every printable character
    /// is text, not a shortcut.
    fn handle_command_bar_key(&mut self, key: KeyEvent) -> Option<Intent> {
        match key.code {
            KeyCode::Char(c) => {
                self.push_command_char(c);
                None
            }
            KeyCode::Backspace => {
                self.pop_command_char();
                None
            }
            KeyCode::Enter => {
                let text = self.take_command_input();
                if text.is_empty() {
                    return None;
                }
                // `Intent::Command` keeps the leading slash — see its own
                // doc comment's example, `/plan` — matching every other
                // place in this file that builds one (`/claim`, `/lexicon`,
                // and so on).
                if text.starts_with('/') {
                    Some(Intent::Command(text))
                } else {
                    Some(Intent::Submit(text))
                }
            }
            _ => None,
        }
    }

    /// Bindings while a pane, not the command bar, has focus.
    fn handle_pane_key(&mut self, key: KeyEvent) -> Option<Intent> {
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        match c {
            't' => {
                self.toggle_thinking();
                None
            }
            'c' => Some(Intent::Command("/claim".to_owned())),
            'r' => Some(Intent::Command("/resolve".to_owned())),
            'f' => Some(Intent::Command("/fog".to_owned())),
            '/' => {
                self.set_focus(Focus::Command);
                self.push_command_char('/');
                None
            }
            '?' => {
                self.toggle_help();
                None
            }
            _ => None,
        }
    }

    /// Handles a mouse click at `(x, y)`, hit-testing against the zones the
    /// most recent frame registered.
    ///
    /// Only [`MouseEventKind::Down`] acts; a drag or a release does nothing,
    /// since every zone in this shell is a single click target.
    #[must_use]
    pub fn handle_mouse(&mut self, x: u16, y: u16, kind: MouseEventKind) -> Option<Intent> {
        if !matches!(kind, MouseEventKind::Down(_)) {
            return None;
        }
        let zone = self.zones().hit_test(x, y)?;
        self.activate_zone(zone)
    }

    /// Performs the action that a zone names, whether a mouse click or its
    /// matching key press triggered it.
    fn activate_zone(&mut self, zone: ZoneId) -> Option<Intent> {
        match zone {
            ZoneId::LeftPane => {
                self.set_focus(Focus::Left);
                None
            }
            ZoneId::RightPane => {
                self.set_focus(Focus::Right);
                None
            }
            ZoneId::CommandBar => {
                self.set_focus(Focus::Command);
                None
            }
            ZoneId::FunctionKey(1) => {
                self.toggle_help();
                None
            }
            ZoneId::FunctionKey(2) => {
                self.set_left_pane(crate::app::pane::LeftPane::Map);
                None
            }
            ZoneId::FunctionKey(3) => {
                self.set_right_pane(crate::app::pane::RightPane::Transcript);
                None
            }
            ZoneId::FunctionKey(4) => {
                self.set_right_pane(crate::app::pane::RightPane::Diff);
                None
            }
            ZoneId::FunctionKey(5) => {
                self.set_right_pane(crate::app::pane::RightPane::Explore);
                None
            }
            ZoneId::FunctionKey(6) => Some(Intent::Command("/lexicon".to_owned())),
            ZoneId::FunctionKey(7) => Some(Intent::Command("/ticket".to_owned())),
            ZoneId::FunctionKey(8) => Some(Intent::Command("/resolve".to_owned())),
            ZoneId::FunctionKey(9) => {
                self.toggle_menu();
                None
            }
            ZoneId::FunctionKey(10) => {
                self.request_quit();
                Some(Intent::Quit)
            }
            // No number in this shell's function-key bar reaches 11, but a
            // click can still land on an unregistered slot, and `ZoneId::Header`
            // is a zone only so it can be hit-tested at all — clicking the
            // title bar names no action.
            ZoneId::Header | ZoneId::FunctionKey(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorLevel, Theme};
    use dark_contract::{Event, Received};
    use ratatui::crossterm::event::{KeyEventState, MouseButton};

    fn app() -> App {
        App::new(Theme::new(ColorLevel::TrueColor))
    }

    fn press(code: KeyCode) -> KeyEvent {
        press_with(code, KeyModifiers::NONE)
    }

    fn press_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn release(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn a_key_release_does_nothing() {
        let mut app = app();
        assert_eq!(app.handle_key(release(KeyCode::F(10))), None);
        assert!(!app.should_quit());
    }

    #[test]
    fn tab_cycles_focus() {
        let mut app = app();
        assert_eq!(app.focus(), Focus::Left);
        let _ = app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.focus(), Focus::Right);
        let _ = app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.focus(), Focus::Command);
        let _ = app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.focus(), Focus::Left);
    }

    #[test]
    fn ctrl_right_cycles_the_focused_pane() {
        let mut app = app();
        assert_eq!(app.left_pane(), crate::app::pane::LeftPane::Map);
        let _ = app.handle_key(press_with(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(app.left_pane(), crate::app::pane::LeftPane::Files);
    }

    #[test]
    fn ctrl_right_on_the_right_pane_leaves_the_left_pane_alone() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Tab)); // focus the right pane
        let _ = app.handle_key(press_with(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(app.left_pane(), crate::app::pane::LeftPane::Map);
        assert_eq!(app.right_pane(), crate::app::pane::RightPane::Diff);
    }

    #[test]
    fn function_keys_jump_straight_to_a_pane() {
        let mut app = app();
        assert_eq!(app.handle_key(press(KeyCode::F(4))), None);
        assert_eq!(app.right_pane(), crate::app::pane::RightPane::Diff);
    }

    #[test]
    fn f10_requests_a_quit() {
        let mut app = app();
        assert_eq!(app.handle_key(press(KeyCode::F(10))), Some(Intent::Quit));
        assert!(app.should_quit());
    }

    #[test]
    fn f9_and_ctrl_p_both_toggle_the_menu() {
        let mut app = app();
        assert!(!app.is_menu_visible());
        let _ = app.handle_key(press(KeyCode::F(9)));
        assert!(app.is_menu_visible());
        let _ = app.handle_key(press_with(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(!app.is_menu_visible());
    }

    #[test]
    fn ctrl_d_asks_to_flip_the_current_dark_state() {
        let mut app = app();
        assert_eq!(
            app.handle_key(press_with(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(Intent::GoDark(true))
        );
        app.apply_event(
            Received::Event(Event::DarkChanged { dark: true }),
            std::time::Instant::now(),
        );
        assert_eq!(
            app.handle_key(press_with(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(Intent::GoDark(false))
        );
    }

    #[test]
    fn esc_cancels_a_running_turn_first() {
        let mut app = app();
        app.apply_event(
            Received::Event(Event::TurnStart {
                turn: "t1".into(),
                class: dark_contract::RoleClass::Worker,
                model: "m".into(),
            }),
            std::time::Instant::now(),
        );
        assert_eq!(app.handle_key(press(KeyCode::Esc)), Some(Intent::Cancel));
    }

    #[test]
    fn esc_closes_help_when_no_turn_is_running() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::F(1)));
        assert!(app.is_help_visible());
        assert_eq!(app.handle_key(press(KeyCode::Esc)), None);
        assert!(!app.is_help_visible());
    }

    #[test]
    fn ctrl_c_outside_a_turn_quits_immediately() {
        let mut app = app();
        assert_eq!(
            app.handle_key(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Intent::Quit)
        );
        assert!(app.should_quit());
    }

    #[test]
    fn ctrl_c_during_a_turn_needs_two_presses() {
        let mut app = app();
        app.apply_event(
            Received::Event(Event::TurnStart {
                turn: "t1".into(),
                class: dark_contract::RoleClass::Worker,
                model: "m".into(),
            }),
            std::time::Instant::now(),
        );

        let first = app.handle_key(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(first, Some(Intent::Cancel));
        assert!(!app.should_quit());

        let second = app.handle_key(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(second, Some(Intent::Quit));
        assert!(app.should_quit());
    }

    #[test]
    fn a_new_turn_disarms_a_pending_quit_confirmation() {
        let mut app = app();
        let now = std::time::Instant::now();
        app.apply_event(
            Received::Event(Event::TurnStart {
                turn: "t1".into(),
                class: dark_contract::RoleClass::Worker,
                model: "m".into(),
            }),
            now,
        );
        let _ = app.handle_key(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.is_quit_armed());

        app.apply_event(
            Received::Event(Event::TurnStart {
                turn: "t2".into(),
                class: dark_contract::RoleClass::Worker,
                model: "m".into(),
            }),
            now,
        );
        assert!(!app.is_quit_armed());
    }

    #[test]
    fn typing_in_the_command_bar_never_triggers_a_pane_shortcut() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Tab));
        let _ = app.handle_key(press(KeyCode::Tab)); // command bar
        assert_eq!(app.focus(), Focus::Command);

        let _ = app.handle_key(press(KeyCode::Char('t')));
        let _ = app.handle_key(press(KeyCode::Char('c')));

        assert_eq!(app.command_input(), "tc");
        assert!(!app.is_thinking_expanded());
    }

    #[test]
    fn enter_with_a_slash_prefix_sends_a_command() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Tab));
        let _ = app.handle_key(press(KeyCode::Tab));
        for c in "/plan".chars() {
            let _ = app.handle_key(press(KeyCode::Char(c)));
        }
        let intent = app.handle_key(press(KeyCode::Enter));
        assert_eq!(intent, Some(Intent::Command("/plan".to_owned())));
        assert_eq!(app.command_input(), "");
    }

    #[test]
    fn enter_without_a_slash_prefix_submits_the_text() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Tab));
        let _ = app.handle_key(press(KeyCode::Tab));
        for c in "hello".chars() {
            let _ = app.handle_key(press(KeyCode::Char(c)));
        }
        let intent = app.handle_key(press(KeyCode::Enter));
        assert_eq!(intent, Some(Intent::Submit("hello".to_owned())));
    }

    #[test]
    fn enter_on_an_empty_command_bar_sends_nothing() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Tab));
        let _ = app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.handle_key(press(KeyCode::Enter)), None);
    }

    #[test]
    fn backspace_removes_the_last_character() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Tab));
        let _ = app.handle_key(press(KeyCode::Tab));
        let _ = app.handle_key(press(KeyCode::Char('a')));
        let _ = app.handle_key(press(KeyCode::Char('b')));
        let _ = app.handle_key(press(KeyCode::Backspace));
        assert_eq!(app.command_input(), "a");
    }

    #[test]
    fn t_toggles_thinking_outside_the_command_bar() {
        let mut app = app();
        assert!(!app.is_thinking_expanded());
        let _ = app.handle_key(press(KeyCode::Char('t')));
        assert!(app.is_thinking_expanded());
        let _ = app.handle_key(press(KeyCode::Char('t')));
        assert!(!app.is_thinking_expanded());
    }

    #[test]
    fn slash_switches_focus_to_the_command_bar_with_the_slash_already_typed() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Char('/')));
        assert_eq!(app.focus(), Focus::Command);
        assert_eq!(app.command_input(), "/");
    }

    #[test]
    fn claim_resolve_and_fog_send_their_commands() {
        let mut app = app();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('c'))),
            Some(Intent::Command("/claim".to_owned()))
        );
        assert_eq!(
            app.handle_key(press(KeyCode::Char('r'))),
            Some(Intent::Command("/resolve".to_owned()))
        );
        assert_eq!(
            app.handle_key(press(KeyCode::Char('f'))),
            Some(Intent::Command("/fog".to_owned()))
        );
    }

    #[test]
    fn question_mark_toggles_help() {
        let mut app = app();
        let _ = app.handle_key(press(KeyCode::Char('?')));
        assert!(app.is_help_visible());
    }

    #[test]
    fn clicking_a_function_key_zone_matches_pressing_that_key() {
        use crate::app::zone::ZoneId;
        let mut app = app();
        app.zones_mut().register(
            ratatui::layout::Rect::new(0, 23, 5, 1),
            ZoneId::FunctionKey(4),
        );
        let intent = app.handle_mouse(1, 23, MouseEventKind::Down(MouseButton::Left));
        assert_eq!(intent, None);
        assert_eq!(app.right_pane(), crate::app::pane::RightPane::Diff);
    }

    #[test]
    fn clicking_outside_every_zone_does_nothing() {
        let mut app = app();
        assert_eq!(
            app.handle_mouse(0, 0, MouseEventKind::Down(MouseButton::Left)),
            None
        );
    }

    #[test]
    fn a_mouse_drag_never_activates_a_zone() {
        use crate::app::zone::ZoneId;
        let mut app = app();
        app.zones_mut()
            .register(ratatui::layout::Rect::new(0, 0, 5, 1), ZoneId::CommandBar);
        assert_eq!(
            app.handle_mouse(1, 0, MouseEventKind::Drag(MouseButton::Left)),
            None
        );
        assert_eq!(app.focus(), Focus::Left);
    }
}
