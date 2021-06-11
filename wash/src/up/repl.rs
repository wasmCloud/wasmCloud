use super::*;
use crate::ctl::{StartActorCommand, UpdateActorCommand};
use crate::util::{Output, Result, WASH_CMD_INFO, WASH_LOG_INFO};
use crossbeam_channel::Sender;
use hotwatch::{Event, Hotwatch};
use log::{debug, error, info};
use std::fs::metadata;
use std::sync::{Arc, Mutex};
use structopt::StructOpt;
use termion::event::Key;
use termion::{raw::RawTerminal, screen::AlternateScreen};
use tui::{
    layout::{Constraint, Direction, Layout},
    Terminal,
};

type ReplTermionBackend =
    tui::backend::TermionBackend<AlternateScreen<RawTerminal<std::io::Stdout>>>;

const REPL_INIT: &str = " REPL (Initializing...) ";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InputState {
    pub(crate) history: Vec<Vec<char>>,
    pub(crate) history_cursor: usize,
    pub(crate) history_offset: u16,
    pub(crate) input: Vec<char>,
    pub(crate) input_cursor: usize,
    pub(crate) input_width: usize,
    pub(crate) focused: bool,
    pub(crate) title: String,
}

impl Default for InputState {
    fn default() -> Self {
        InputState {
            history: vec![],
            history_cursor: 0,
            history_offset: 0,
            input: vec![],
            input_cursor: 0,
            input_width: 40,
            focused: true,
            title: REPL_INIT.to_string(),
        }
    }
}

impl InputState {
    pub(crate) fn cursor_location(&mut self) -> (u16, u16) {
        let mut position = (0, 0);

        position.0 += WASH_PROMPT.len();

        for _c in 0..self.input_cursor {
            position.0 += 1;
            if position.0 == self.input_width {
                position.0 = 0;
                position.1 += 1;
            }
        }

        // Offset Y by length of command history and multiline history
        position.1 += self.vertical_history_offset();

        (position.0 as u16, position.1 as u16)
    }

    /// Computes vertical offset from command history
    pub(crate) fn vertical_history_offset(&mut self) -> u16 {
        self.history_offset = self
            .history
            .iter()
            .map(|h| {
                let input_length = h.len() + WASH_PROMPT.len();
                let multilines = input_length / self.input_width;
                if multilines >= 1 && input_length != self.input_width {
                    1_u16 + multilines as u16
                } else {
                    1_u16
                }
            })
            .sum();
        self.history_offset
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OutputState {
    pub(crate) output: Vec<String>,
    pub(crate) output_cursor: usize,
    pub(crate) output_width: usize,
    pub(crate) output_scroll: u16,
}

impl Default for OutputState {
    fn default() -> Self {
        OutputState {
            output: vec![],
            output_cursor: 0,
            output_width: 80,
            output_scroll: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ReplMode {
    Standalone,
    Lattice,
}

#[derive(Debug, Clone)]
pub(crate) struct EmbeddedHost {
    pub(crate) id: String,
    pub(crate) mode: ReplMode,
    pub(crate) op_sender: Sender<CtlCliCommand>,
}

impl EmbeddedHost {
    pub(crate) fn new(id: String, mode: ReplMode, op_sender: Sender<CtlCliCommand>) -> Self {
        EmbeddedHost {
            id,
            mode,
            op_sender,
        }
    }

    /// Issues appropriate `ctl` commands to start the provided actors and create a
    /// `Hotwatch` object for each of them. This `Hotwatch` object is responsible
    /// for detecting write events and issuing a `CtlCliCommand::Update` when
    /// an update occurs
    pub(crate) fn watch_actors(&self, actors: Vec<PathBuf>) -> Vec<Hotwatch> {
        actors
            .iter()
            .filter_map(|actor| match actor.to_str() {
                Some(path) if std::fs::metadata(actor).is_ok() => match self.watch_actor(path) {
                    Ok(hw) => Some(hw),
                    Err(e) => {
                        error!(target: WASH_CMD_INFO, "Unable to watch actor: {}", e);
                        None
                    }
                },
                _ => {
                    error!(
                        target: WASH_CMD_INFO,
                        "Unable to watch actor: file does not exist"
                    );
                    None
                }
            })
            .collect::<Vec<_>>()
    }

    fn watch_actor(&self, actor_ref: &str) -> Result<Hotwatch> {
        debug!(
            target: WASH_CMD_INFO,
            "Attempting to watch actor {}", actor_ref
        );
        let start_cmd = CtlCliCommand::Start(StartCommand::Actor(StartActorCommand::new(
            ConnectionOpts::default(),
            Output::default(),
            Some(self.id.to_string()),
            actor_ref.to_string(),
            None,
            1,
        )));
        self.op_sender.send(start_cmd)?;

        let actor = Actor::from_file(&actor_ref).map_err(convert_error)?;

        // Repeated updates with the same actor can re-use the same command
        let update_cmd = CtlCliCommand::Update(UpdateCommand::Actor(UpdateActorCommand::new(
            ConnectionOpts::default(),
            Output::default(),
            self.id.to_string(),
            actor.public_key(),
            actor_ref.to_string(),
        )));

        let op_sender = self.op_sender.clone();
        let actor_ref = actor_ref.to_string();
        let mut hotwatch = Hotwatch::new().expect("hotwatch failed to initialize!");
        hotwatch
            .watch(actor_ref.clone(), move |event: Event| {
                // Watching for Event::NoticeWrite is faster, but it induces a race condition where
                // an update _could_ take place _before_ the write has finished.
                if let Event::Write(_path) = event {
                    info!(
                        target: WASH_CMD_INFO,
                        "Detected actor change for actor {}, updating", actor_ref
                    );
                    let _ = op_sender.send(update_cmd.clone());
                }
            })
            .expect("failed to watch file");

        Ok(hotwatch)
    }
}

pub(crate) struct WashRepl {
    pub(crate) input_state: InputState,
    pub(crate) output_state: Arc<Mutex<OutputState>>,
    pub(crate) tui_state: TuiWidgetState,
    pub(crate) embedded_host: Option<EmbeddedHost>,
}

impl Default for WashRepl {
    fn default() -> Self {
        WashRepl {
            input_state: InputState::default(),
            output_state: Arc::new(Mutex::new(OutputState::default())),
            tui_state: TuiWidgetState::new(),
            embedded_host: None,
        }
    }
}

impl WashRepl {
    /// Using the state of the REPL, display information in the terminal window
    pub(crate) fn draw_ui(&mut self, terminal: &mut Terminal<ReplTermionBackend>) -> Result<()> {
        terminal.draw(|frame| {
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(67), Constraint::Min(5)].as_ref())
                .split(frame.size());

            let io_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(40), Constraint::Min(10)])
                .split(main_chunks[0]);

            draw_input_panel(frame, &mut self.input_state, io_chunks[0]);
            draw_output_panel(
                frame,
                Arc::clone(&self.output_state),
                io_chunks[1],
                self.input_state.focused,
            );
            draw_smart_logger(
                frame,
                main_chunks[1],
                &self.tui_state,
                !self.input_state.focused,
            );
        })?;
        Ok(())
    }

    /// Handles key input by the user into the REPL
    pub(crate) async fn handle_key_event(&mut self, key: Key) -> Result<()> {
        match key {
            Key::PageUp => {
                let mut state = self.output_state.lock().unwrap();
                if state.output_cursor > 0 && state.output_scroll > 0 {
                    state.output_cursor -= 1;
                }
            }
            Key::PageDown => {
                let mut state = self.output_state.lock().unwrap();
                if state.output_cursor < state.output.len() {
                    state.output_cursor += 1;
                }
            }
            Key::Left => {
                if self.input_state.input_cursor > 0 {
                    self.input_state.input_cursor -= 1
                }
            }
            Key::Right => {
                if self.input_state.input_cursor < self.input_state.input.len() {
                    self.input_state.input_cursor += 1
                }
            }
            Key::Up => {
                if self.input_state.history_cursor > 0 {
                    self.input_state.history_cursor -= 1;
                    self.input_state.input =
                        self.input_state.history[self.input_state.history_cursor].clone();
                    self.input_state.input_cursor = self.input_state.input.len();
                }
            }
            Key::Down => {
                if self.input_state.history.is_empty() {
                    return Ok(());
                };
                if self.input_state.history_cursor < self.input_state.history.len() - 1 {
                    self.input_state.history_cursor += 1;
                    self.input_state.input =
                        self.input_state.history[self.input_state.history_cursor].clone();
                    self.input_state.input_cursor = self.input_state.input.len();
                } else if self.input_state.history_cursor >= self.input_state.history.len() - 1 {
                    self.input_state.history_cursor = self.input_state.history.len();
                    self.input_state.input.clear();
                    self.input_state.input_cursor = 0;
                }
            }
            Key::Backspace => {
                if self.input_state.input_cursor > 0
                    && self.input_state.input_cursor <= self.input_state.input.len()
                {
                    self.input_state.input_cursor -= 1;
                    self.input_state.input.remove(self.input_state.input_cursor);
                };
            }
            //TODO(issue #67): navigate left one word
            // Key::Alt(c) if c == 'b' => {
            //     ()
            // }
            //TODO(issue #67): navigate right one word
            // Key::Alt(c) if c == 'f' => {
            //     ()
            // }
            Key::Char(c) if c == '\n' => {
                let cmd: String = self.input_state.input.iter().collect();
                let iter = cmd.split_ascii_whitespace();

                crate::util::set_max_text_output_width(
                    self.output_state.lock().unwrap().output_width - 2,
                );
                let cli = ReplCli::from_iter_safe(iter);

                self.input_state
                    .history
                    .push(self.input_state.input.clone());
                self.input_state.history_cursor = self.input_state.history.len();
                self.input_state.input.clear();
                self.input_state.input_cursor = 0;

                match cli {
                    Ok(ReplCli { cmd }) => {
                        use ReplCliCommand::*;
                        match cmd {
                            Clear => {
                                info!(target: WASH_LOG_INFO, "Clearing REPL history");
                                self.input_state = InputState::default();
                            }
                            Quit => {
                                info!(target: WASH_CMD_INFO, "Goodbye");
                                return Err("REPL Quit".into());
                            }
                            ReplCliCommand::Drain(draincmd) => {
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    match handle_drain(draincmd, output_state) {
                                        Ok(r) => r,
                                        Err(e) => error!("Error handling drain: {}", e),
                                    };
                                });
                            }
                            ReplCliCommand::Claims(claimscmd) => {
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    let rt = actix_rt::System::new();
                                    rt.block_on(async {
                                        match handle_claims(claimscmd, output_state).await {
                                            Ok(r) => r,
                                            Err(e) => error!("Error handling claims: {}", e),
                                        };
                                    });
                                });
                            }
                            ReplCliCommand::Ctl(ctlcmd) => {
                                // This match statement handles loading an actor from disk instead of from an OCI registry
                                //
                                // When a Start or Update Actor `ctl` command is sent, we send the `ctl` command to the host API for the following cases:
                                // 1. The Host is running in standalone mode (all ctl commands are delegated to host API)
                                // 2. The actor_ref exists as a file on disk AND:
                                //    a. The host ID specified is the embedded host
                                //    b. The host ID is not specified (the embedded host is a suitable host for a local actor)
                                match (self.embedded_host.as_ref(), ctlcmd.clone()) {
                                    (
                                        Some(host),
                                        CtlCliCommand::Start(StartCommand::Actor(cmd)),
                                    ) if host.mode == ReplMode::Lattice => {
                                        if metadata(&cmd.actor_ref).is_ok() // File exists
                                            && (cmd.host_id.is_none()
                                                || cmd.host_id.unwrap() == host.id)
                                        {
                                            host.op_sender.send(ctlcmd)?;
                                            return Ok(());
                                        }
                                    }
                                    (
                                        Some(host),
                                        CtlCliCommand::Update(UpdateCommand::Actor(cmd))
                                    ) if host.mode == ReplMode::Lattice => {
                                        if metadata(&cmd.new_actor_ref).is_ok() // File exists
                                            && cmd.host_id == host.id
                                        {
                                            host.op_sender.send(ctlcmd)?;
                                            return Ok(());
                                        }
                                    }
                                    (Some(host), cmd) if host.mode == ReplMode::Standalone => {
                                        host.op_sender.send(cmd)?;
                                        return Ok(());
                                    }
                                    _ => debug!("Dispatching command to lattice control interface (actor not found locally)"),
                                }
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    let rt = actix_rt::System::new();
                                    rt.block_on(async {
                                        match handle_ctl(ctlcmd, output_state).await {
                                            Ok(r) => r,
                                            Err(e) => error!("Error handling ctl: {}", e),
                                        };
                                    });
                                });
                            }
                            ReplCliCommand::Keys(keyscmd) => {
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    let rt = actix_rt::System::new();
                                    rt.block_on(async {
                                        match handle_keys(keyscmd, output_state).await {
                                            Ok(r) => r,
                                            Err(e) => error!("Error handling key: {}", e),
                                        };
                                    });
                                });
                            }
                            ReplCliCommand::Par(parcmd) => {
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    let rt = actix_rt::System::new();
                                    rt.block_on(async {
                                        match handle_par(parcmd, output_state).await {
                                            Ok(r) => r,
                                            Err(e) => error!("Error handling par: {}", e),
                                        };
                                    });
                                });
                            }
                            ReplCliCommand::Reg(regcmd) => {
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    let rt = actix_rt::System::new();
                                    rt.block_on(async {
                                        match handle_reg(regcmd, output_state).await {
                                            Ok(r) => r,
                                            Err(e) => error!("Error handling reg: {}", e),
                                        };
                                    });
                                });
                            }
                        }
                    }
                    Err(e) => {
                        use structopt::clap::ErrorKind::*;
                        // HelpDisplayed is the StructOpt help text error, which should be displayed as info
                        const WASH_HELP: &str = "WASH_HELP";
                        match e.kind {
                            HelpDisplayed => {
                                for line in e.message.split('\n') {
                                    if !line.is_empty() {
                                        info!(target: WASH_HELP, " {}", line);
                                    } else {
                                        info!(target: WASH_HELP, "\n");
                                    }
                                }
                            }
                            _ => {
                                for line in e.message.split('\n') {
                                    if !line.is_empty() {
                                        error!(target: WASH_HELP, " {}", line)
                                    } else {
                                        error!(target: WASH_HELP, "\n");
                                    }
                                }
                            }
                        }
                    }
                };
            }
            Key::Char(c) => {
                self.input_state
                    .input
                    .insert(self.input_state.input_cursor, c);
                self.input_state.input_cursor += 1;
            }
            _ => (),
        };
        Ok(())
    }

    /// Handles keys sent to the tui_logger
    pub(crate) async fn handle_tui_logger_key_event(&mut self, key: Key) -> Result<()> {
        match key {
            Key::Char(' ') => {
                self.tui_state.transition(&TuiWidgetEvent::SpaceKey);
            }
            Key::Esc => {
                self.tui_state.transition(&TuiWidgetEvent::EscapeKey);
            }
            Key::PageUp => {
                self.tui_state.transition(&TuiWidgetEvent::PrevPageKey);
            }
            Key::PageDown => {
                self.tui_state.transition(&TuiWidgetEvent::NextPageKey);
            }
            Key::Up => {
                self.tui_state.transition(&TuiWidgetEvent::UpKey);
            }
            Key::Down => {
                self.tui_state.transition(&TuiWidgetEvent::DownKey);
            }
            Key::Left => {
                self.tui_state.transition(&TuiWidgetEvent::LeftKey);
            }
            Key::Right => {
                self.tui_state.transition(&TuiWidgetEvent::RightKey);
            }
            Key::Char('+') => {
                self.tui_state.transition(&TuiWidgetEvent::PlusKey);
            }
            Key::Char('-') => {
                self.tui_state.transition(&TuiWidgetEvent::MinusKey);
            }
            Key::Char('h') => {
                self.tui_state.transition(&TuiWidgetEvent::HideKey);
            }
            Key::Char('f') => {
                self.tui_state.transition(&TuiWidgetEvent::FocusKey);
            }
            _ => (),
        }
        Ok(())
    }
}
