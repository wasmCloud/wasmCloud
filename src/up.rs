use crate::claims::*;
use crate::ctl::*;
use crate::drain::*;
use crate::keys::*;
use crate::par::*;
use crate::reg::*;
use crate::util::{convert_error, Result, WASH_CMD_INFO, WASH_LOG_INFO};
use log::{error, info, LevelFilter};
use std::io;
use std::sync::{Arc, Mutex};
use structopt::{clap::AppSettings, StructOpt};
use termion::event::{Event, Key};
use termion::{
    input::TermRead,
    raw::{IntoRawMode, RawTerminal},
    screen::AlternateScreen,
};
use tui::{
    backend::TermionBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use tui_logger::*;
use wasmcloud_host::HostBuilder;

type REPLTermionBackend =
    tui::backend::TermionBackend<AlternateScreen<RawTerminal<std::io::Stdout>>>;

const CTL_NS: &str = "default";
const WASH_PROMPT: &str = "wash> ";
/// Option is unsupported for MacOS, the following byte slices correspond
/// to [1;3A for Option+UP and [1;3B for Option+Down
const OPTIONUP: &[u8] = &[27_u8, 91_u8, 49_u8, 59_u8, 51_u8, 65_u8];
const OPTIONDOWN: &[u8] = &[27_u8, 91_u8, 49_u8, 59_u8, 51_u8, 66_u8];

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "up")]
pub(crate) struct UpCli {
    #[structopt(flatten)]
    command: UpCliCommand,
}

impl UpCli {
    pub(crate) fn command(self) -> UpCliCommand {
        self.command
    }
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct UpCliCommand {
    /// Host for lattice connections, defaults to 0.0.0.0
    #[structopt(
        short = "h",
        long = "host",
        default_value = "0.0.0.0",
        env = "WASH_RPC_HOST"
    )]
    rpc_host: String,

    /// Port for lattice connections, defaults to 4222
    #[structopt(
        short = "p",
        long = "port",
        default_value = "4222",
        env = "WASH_RPC_PORT"
    )]
    rpc_port: String,

    /// Log level verbosity, valid values are `error`, `warn`, `info`, `debug`, and `trace`
    #[structopt(short = "l", long = "log-level", default_value = "debug")]
    log_level: LogLevel,
}

#[derive(StructOpt, Debug, Clone, PartialEq)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl std::str::FromStr for LogLevel {
    type Err = std::io::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "error" => Ok(LogLevel::Error),
            "warn" => Ok(LogLevel::Warn),
            "info" => Ok(LogLevel::Info),
            "debug" => Ok(LogLevel::Debug),
            "trace" => Ok(LogLevel::Trace),
            _ => Ok(LogLevel::Trace),
        }
    }
}

pub(crate) async fn handle_command(command: UpCliCommand) -> Result<()> {
    let UpCliCommand { .. } = command;
    handle_up(command).await
}

#[derive(StructOpt, Debug, Clone)]
#[structopt(name = "wash>", global_settings(&[AppSettings::NoBinaryName, AppSettings::DisableVersion, AppSettings::ColorNever]))]
struct ReplCli {
    #[structopt(flatten)]
    cmd: ReplCliCommand,
}

#[derive(StructOpt, Debug, Clone)]
#[structopt(global_settings(&[AppSettings::ColorNever, AppSettings::DisableVersion, AppSettings::VersionlessSubcommands]))]
enum ReplCliCommand {
    // Manage contents of local wasmcloud cache
    #[structopt(name = "drain")]
    Drain(DrainCliCommand),

    /// Interact with a wasmcloud control interface
    #[structopt(name = "ctl")]
    Ctl(CtlCliCommand),

    /// Generate and manage JWTs for wasmcloud Actors
    #[structopt(name = "claims")]
    Claims(ClaimsCliCommand),

    /// Utilities for generating and managing keys
    #[structopt(name = "keys", aliases = &["key"])]
    Keys(KeysCliCommand),

    /// Create, inspect, and modify capability provider archive files
    #[structopt(name = "par")]
    Par(ParCliCommand),

    /// Launch wasmcloud REPL environment
    #[structopt(name = "reg")]
    Reg(RegCliCommand),

    /// Terminates the REPL environment (also accepts 'exit', 'logout', 'q' and ':q!')
    #[structopt(name = "quit", aliases = &["exit", "logout", "q", ":q!"])]
    Quit,

    /// Clears the REPL input history
    #[structopt(name = "clear")]
    Clear,
}

#[derive(Debug, Clone, PartialEq)]
struct InputState {
    history: Vec<Vec<char>>,
    history_cursor: usize,
    input: Vec<char>,
    input_cursor: usize,
    multiline_history: u16, // amount to offset cursor for multiline inputs
    input_width: usize,
    focused: bool,
}

impl Default for InputState {
    fn default() -> Self {
        InputState {
            history: vec![],
            history_cursor: 0,
            input: vec![],
            input_cursor: 0,
            multiline_history: 0,
            input_width: 40,
            focused: true,
        }
    }
}

impl InputState {
    fn cursor_location(&self) -> (u16, u16) {
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
        position.1 += self.history.len();
        //TODO(issue #90): Multiline history is calculated relative to the current terminal width
        //                 when a terminal is resized, it needs to be re-evaluated
        position.1 += self.multiline_history as usize;

        (position.0 as u16, position.1 as u16)
    }
}

#[derive(Debug, Clone)]
struct OutputState {
    output: Vec<String>,
    output_cursor: usize,
    output_width: usize,
    output_scroll: u16,
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

struct WashRepl {
    input_state: InputState,
    output_state: Arc<Mutex<OutputState>>,
    tui_state: TuiWidgetState,
}

impl Default for WashRepl {
    fn default() -> Self {
        WashRepl {
            input_state: InputState::default(),
            output_state: Arc::new(Mutex::new(OutputState::default())),
            tui_state: TuiWidgetState::new(),
        }
    }
}

impl WashRepl {
    /// Using the state of the REPL, display information in the terminal window
    fn draw_ui(&mut self, terminal: &mut Terminal<REPLTermionBackend>) -> Result<()> {
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
    async fn handle_key_event(&mut self, key: Key) -> Result<()> {
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
                let cli = ReplCli::from_iter_safe(iter);

                let multilines = self.input_state.input.len() / self.input_state.input_width;
                if multilines >= 1 {
                    self.input_state.multiline_history += multilines as u16;
                };

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
                                    let mut rt = actix_rt::System::new("cmd");
                                    rt.block_on(async {
                                        match handle_claims(claimscmd, output_state).await {
                                            Ok(r) => r,
                                            Err(e) => error!("Error handling claims: {}", e),
                                        };
                                    });
                                });
                            }
                            ReplCliCommand::Ctl(ctlcmd) => {
                                let output_state = Arc::clone(&self.output_state);
                                std::thread::spawn(|| {
                                    let mut rt = actix_rt::System::new("cmd");
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
                                    let mut rt = actix_rt::System::new("cmd");
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
                                    let mut rt = actix_rt::System::new("cmd");
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
                                    let mut rt = actix_rt::System::new("cmd");
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
    async fn handle_tui_logger_key_event(&mut self, key: Key) -> Result<()> {
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

/// Launches REPL environment
async fn handle_up(cmd: UpCliCommand) -> Result<()> {
    // Initialize logger at default level based on user input. Defaults to Debug
    // Trace is very noisy and should be used only for intense debugging
    use LogLevel::*;
    let filter = match cmd.log_level {
        Error => LevelFilter::Error,
        Warn => LevelFilter::Warn,
        Info => LevelFilter::Info,
        Debug => LevelFilter::Debug,
        Trace => LevelFilter::Trace,
    };
    init_logger(filter).unwrap();
    set_default_level(filter);

    // Set global variable to show we're in REPL mode
    // This ensures the rest of the modules can properly format output information
    crate::util::REPL_MODE.set("true".to_string()).unwrap();

    // Initialize terminal
    let backend = {
        let stdout = io::stdout().into_raw_mode().unwrap();
        let stdout = AlternateScreen::from(stdout);
        TermionBackend::new(stdout)
    };
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.clear().unwrap();
    terminal.hide_cursor().unwrap();

    // Start REPL
    let mut repl = WashRepl::default();
    repl.draw_ui(&mut terminal)?;
    info!(target: WASH_LOG_INFO, "Initializing REPL...");
    // Sending SPACE event to tui logger to hide disabled logs
    repl.tui_state.transition(&TuiWidgetEvent::SpaceKey);
    repl.draw_ui(&mut terminal)?;

    // Launch host in separate thread to avoid blocking host operations
    std::thread::spawn(move || {
        let mut rt = actix_rt::System::new("replhost");
        rt.block_on(async move {
            let nc_rpc =
                match nats::asynk::connect(&format!("{}:{}", cmd.rpc_host, cmd.rpc_port)).await {
                    Ok(conn) => conn,
                    Err(_e) => {
                        error!(
                            target: WASH_CMD_INFO,
                            "Error connecting to NATS at {}:{}",
                            cmd.rpc_host,
                            cmd.rpc_port
                        );
                        error!(target: WASH_CMD_INFO, "NATS is required to run control interface (ctl) commands. Please refer to
https://www.wasmcloud.dev/overview/getting-started/#starting-nats for instructions on how to launch NATS");
                        return;
                    }
                };
            let nc_control =
                match nats::asynk::connect(&format!("{}:{}", cmd.rpc_host, cmd.rpc_port)).await {
                    Ok(conn) => conn,
                    Err(_e) => {
                        error!(
                            target: WASH_CMD_INFO,
                            "Error connecting to NATS at {}:{}",
                            cmd.rpc_host,
                            cmd.rpc_port
                        );
                        error!(target: WASH_CMD_INFO, "NATS is required to run control interface (ctl) commands. Please refer to
https://www.wasmcloud.dev/overview/getting-started/#starting-nats for instructions on how to launch NATS");
                        return;
                    }
                };
            let host = HostBuilder::new()
                .with_namespace(CTL_NS)
                .with_rpc_client(nc_rpc)
                .with_control_client(nc_control)
                .with_label("repl_mode", "true")
                .oci_allow_latest()
                .oci_allow_insecure(vec!["localhost:5000".to_string()])
                .enable_live_updates()
                .build();
            if let Err(_e) = host.start().await.map_err(convert_error) {
                error!(target: WASH_LOG_INFO, "Error launching REPL host");
            } else {
                info!(
                    target: WASH_LOG_INFO,
                    "Host ({}) started in namespace ({})",
                    host.id(),
                    CTL_NS
                );
            };
            // Since CTRL+C won't be captured by this thread, host will stop when REPL exits
            actix_rt::signal::ctrl_c().await.unwrap();
            host.stop().await;
        });
    });

    repl.draw_ui(&mut terminal)?;

    // Use a channel to asynchronously receive stdin events
    let (tx, rx) = std::sync::mpsc::channel();
    let tx_event = tx.clone();
    std::thread::spawn({
        let stdin = io::stdin();
        move || {
            for c in stdin.events() {
                tx_event.send(c).unwrap();
            }
        }
    });

    loop {
        if let Ok(evt) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
            let res = match evt? {
                // Tab key toggles input focus between REPL and Tui logger selector
                Event::Key(Key::Char('\t')) => {
                    repl.input_state.focused = !repl.input_state.focused;
                    info!(
                        target: WASH_CMD_INFO,
                        "Switched command focus to {}",
                        if repl.input_state.focused {
                            "REPL"
                        } else {
                            "Logger selector"
                        }
                    );
                    Ok(())
                }
                // Dispatch events for REPL interpretation
                Event::Key(event) if repl.input_state.focused => repl.handle_key_event(event).await,
                // Dispatch events for Tui Target interpretation
                Event::Key(event) if !repl.input_state.focused => {
                    repl.handle_tui_logger_key_event(event).await
                }
                // OPTION+Up/Down are unsupported on MacOS, send PageUp / PageDown in their place
                Event::Unsupported(event_bytes) => match event_bytes.as_slice() {
                    OPTIONUP if repl.input_state.focused => {
                        repl.handle_key_event(Key::PageUp).await
                    }
                    OPTIONDOWN if repl.input_state.focused => {
                        repl.handle_key_event(Key::PageDown).await
                    }
                    OPTIONUP if !repl.input_state.focused => {
                        repl.handle_tui_logger_key_event(Key::PageUp).await
                    }
                    OPTIONDOWN if !repl.input_state.focused => {
                        repl.handle_tui_logger_key_event(Key::PageDown).await
                    }
                    _ => Ok(()),
                },
                _ => Ok(()),
            };
            repl.draw_ui(&mut terminal)?;

            // Exit the terminal gracefully
            if res.is_err() {
                cleanup_terminal(&mut terminal);
                break;
            }
        } else {
            repl.draw_ui(&mut terminal)?;
        }
    }
    cleanup_terminal(&mut terminal);
    Ok(())
}

fn handle_drain(drain_cmd: DrainCliCommand, output_state: Arc<Mutex<OutputState>>) -> Result<()> {
    let output = crate::drain::handle_command(drain_cmd)?;
    log_to_output(output_state, output);
    Ok(())
}

async fn handle_claims(
    claims_cmd: ClaimsCliCommand,
    output_state: Arc<Mutex<OutputState>>,
) -> Result<()> {
    let output = crate::claims::handle_command(claims_cmd).await?;
    log_to_output(output_state, output);
    Ok(())
}

async fn handle_ctl(ctl_cmd: CtlCliCommand, output_state: Arc<Mutex<OutputState>>) -> Result<()> {
    let output = crate::ctl::handle_command(ctl_cmd).await?;
    log_to_output(output_state, output);
    Ok(())
}

async fn handle_keys(
    keys_cmd: KeysCliCommand,
    output_state: Arc<Mutex<OutputState>>,
) -> Result<()> {
    let output = crate::keys::handle_command(keys_cmd)?;
    log_to_output(output_state, output);
    Ok(())
}

async fn handle_par(par_cmd: ParCliCommand, output_state: Arc<Mutex<OutputState>>) -> Result<()> {
    let output = crate::par::handle_command(par_cmd).await?;
    log_to_output(output_state, output);
    Ok(())
}

async fn handle_reg(reg_cmd: RegCliCommand, output_state: Arc<Mutex<OutputState>>) -> Result<()> {
    let output = crate::reg::handle_command(reg_cmd).await?;
    log_to_output(output_state, output);
    Ok(())
}

/// Helper function to exit the alternate tui terminal without corrupting the user terminal
fn cleanup_terminal(terminal: &mut Terminal<REPLTermionBackend>) {
    terminal.show_cursor().unwrap();
    terminal.clear().unwrap();
}

/// Append a message to the output log
fn log_to_output(state: Arc<Mutex<OutputState>>, out: String) {
    // Reset output scroll to bottom
    let mut state = state.lock().unwrap();
    state.output_cursor = state.output.len();

    let output_width = state.output_width - 2;

    // Newlines are used here for accurate scrolling in the Output pane
    out.split('\n').for_each(|line| {
        let line_len = line.chars().count();
        if line_len > output_width {
            let mut offset = 0;
            // Div and round up
            let n_lines = (line_len + (output_width - 1)) / output_width;
            for _ in 0..n_lines {
                let sub_line = line.chars().skip(offset).take(output_width).collect();
                state.output.push(sub_line);
                offset += output_width
            }
            state.output_cursor += n_lines;
        } else {
            state.output.push(line.to_string());
            state.output_cursor += 1;
        }
    });
    state.output.push("".to_string());
    state.output_cursor += 1;
}

/// Helper function to delimit an input vec by newlines for proper REPL display
fn format_input_for_display(input_vec: Vec<char>, input_width: usize) -> String {
    let mut input = String::new();
    let mut index = WASH_PROMPT.len() - 1;
    let disp_iter = input_vec.iter();
    for c in disp_iter {
        if index == input_width - 1 {
            input.push('\n');
            input.push(*c);
            index = 0;
        } else {
            input.push(*c);
            index += 1;
        }
    }
    input
}

/// Display the wash REPL in the provided panel, automatically scroll with overflow
fn draw_input_panel(frame: &mut Frame<REPLTermionBackend>, state: &mut InputState, chunk: Rect) {
    let history: String = state
        .history
        .iter()
        .map(|h| {
            format!(
                "{}{}\n",
                WASH_PROMPT,
                format_input_for_display(h.to_vec(), state.input_width)
            )
        })
        .collect();
    let prompt: String = WASH_PROMPT.to_string();

    let display = format!(
        "{}{}{}",
        history,
        prompt,
        format_input_for_display(state.input.clone(), state.input_width)
    );

    // 5 is the offset from the bottom of the chunk (3) plus 2 lines for buffer
    let scroll_offset = if state.history.len() as u16 + state.multiline_history >= chunk.height - 3
    {
        state.multiline_history + state.history.len() as u16 + 5 - chunk.height
    } else {
        0
    };
    // 3 is chunk size minus borders minus buffer space
    state.input_width = chunk.width as usize - 3;

    let style = if state.focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    // Draw REPL panel
    let input_panel = Paragraph::new(display)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" REPL ", style)),
        )
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Left)
        .scroll((scroll_offset, 0));
    frame.render_widget(input_panel, chunk);

    let input_cursor = state.cursor_location();

    // Draw cursor on screen
    frame.set_cursor(
        chunk.x + 1 + input_cursor.0,
        chunk.y + 1 + input_cursor.1 - scroll_offset,
    )
}

/// Display command output in the provided panel
fn draw_output_panel(
    frame: &mut Frame<REPLTermionBackend>,
    state: Arc<Mutex<OutputState>>,
    chunk: Rect,
    focused: bool,
) {
    let mut state = state.lock().unwrap();
    let output_logs: String = state.output.iter().map(|h| format!(" {}\n", h)).collect();

    // Autoscroll if output overflows chunk height, adjusting for manual scroll with output_cursor
    let output_length = state.output.len() as u16;
    let output_cursor = state.output_cursor as u16;
    state.output_scroll = if output_length >= chunk.height - 3 {
        if output_cursor >= chunk.height {
            output_cursor as u16 + 1 - chunk.height
        } else {
            0
        }
    } else {
        0
    };
    state.output_width = chunk.width as usize - 1;

    let style = if focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    // Draw REPL panel
    let output_panel = Paragraph::new(output_logs)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " OUTPUT (ALT+UP/DOWN or PageUp/PageDown to scroll) ",
            style,
        )))
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Left)
        .scroll((state.output_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(output_panel, chunk);
}

/// Draws the Tui smart logger widget in the provided frame
fn draw_smart_logger(
    frame: &mut Frame<REPLTermionBackend>,
    chunk: Rect,
    state: &TuiWidgetState,
    focused: bool,
) {
    let style = if focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let selector_panel = TuiLoggerSmartWidget::default()
        .title_log(" Tui Log ")
        .title_target(" Tui Target Selector ")
        .style_error(Style::default().fg(Color::Red))
        .style_debug(Style::default().fg(Color::Green))
        .style_warn(Style::default().fg(Color::Yellow))
        .style_trace(Style::default().fg(Color::Magenta))
        .style_info(Style::default().fg(Color::Cyan))
        .border_style(style)
        .state(state);
    // These loggers are far too noisy and don't provide any value to a wasmcloud user
    set_level_for_target("tui_logger::dispatcher", LevelFilter::Off);
    set_level_for_target("mio::poll", LevelFilter::Off);
    set_level_for_target("mio::sys::unix::kqueue", LevelFilter::Off);
    set_level_for_target("polling", LevelFilter::Off);
    set_level_for_target("polling::kqueue", LevelFilter::Off);
    set_level_for_target("async_io::driver", LevelFilter::Off);
    set_level_for_target("async_io::reactor", LevelFilter::Off);

    frame.render_widget(selector_panel, chunk);
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    /// Enumerates multiple options of the `up` command to ensure API doesn't
    /// change between versions. This test will fail if `wash up`
    /// changes syntax, ordering of required elements, or flags.
    fn test_up_comprehensive() -> Result<()> {
        const LOG_LEVEL: &str = "info";
        const RPC_HOST: &str = "0.0.0.0";
        const RPC_PORT: &str = "4222";

        let up_all_options = UpCli::from_iter_safe(&[
            "up",
            "--log-level",
            LOG_LEVEL,
            "--host",
            RPC_HOST,
            "--port",
            RPC_PORT,
        ])?;
        let up_all_short_options =
            UpCli::from_iter_safe(&["up", "-l", LOG_LEVEL, "-h", RPC_HOST, "-p", RPC_PORT])?;

        #[allow(unreachable_patterns)]
        match up_all_options.command {
            UpCliCommand {
                rpc_host,
                rpc_port,
                log_level,
            } => {
                assert_eq!(rpc_host, RPC_HOST);
                assert_eq!(rpc_port, RPC_PORT);
                assert_eq!(log_level, LogLevel::Info);
            }
            cmd => panic!("up generated other command {:?}", cmd),
        }

        #[allow(unreachable_patterns)]
        match up_all_short_options.command {
            UpCliCommand {
                rpc_host,
                rpc_port,
                log_level,
            } => {
                assert_eq!(rpc_host, RPC_HOST);
                assert_eq!(rpc_port, RPC_PORT);
                assert_eq!(log_level, LogLevel::Info);
            }
            cmd => panic!("up generated other command {:?}", cmd),
        }

        Ok(())
    }

    #[test]
    fn test_up_input_format() {
        const CALL_INPUT: &str = "ctl call MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5 HandleRequest {\"method\": \"GET\", \"path\": \"/\", \"body\": \"\", \"queryString\":\"\", \"header\":{}}";
        const START_ACTOR_INPUT: &str = "ctl start actor wasmcloud.azurecr.io/echo:0.2.0";
        const LINK_INPUT: &str = "ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M wasmcloud:httpserver PORT=8080";
        const TERMINAL_WIDTH: usize = 80;
        let prompt_length = super::WASH_PROMPT.len(); // `wash> `

        let (call_first_line, call_second_line) =
            CALL_INPUT.split_at(TERMINAL_WIDTH - prompt_length);
        let call_input_display =
            format_input_for_display(CALL_INPUT.chars().collect(), TERMINAL_WIDTH);
        let mut call_iter = call_input_display.split('\n');
        assert_eq!(call_first_line, call_iter.next().unwrap());
        assert_eq!(call_second_line, call_iter.next().unwrap());

        assert!(START_ACTOR_INPUT.len() < TERMINAL_WIDTH - prompt_length);
        let start_input_display =
            format_input_for_display(START_ACTOR_INPUT.chars().collect(), TERMINAL_WIDTH);
        let mut start_iter = start_input_display.split('\n');
        assert_eq!(START_ACTOR_INPUT, start_iter.next().unwrap());

        let (link_first_line, link_second_line) =
            LINK_INPUT.split_at(TERMINAL_WIDTH - prompt_length);
        let link_input_display =
            format_input_for_display(LINK_INPUT.chars().collect(), TERMINAL_WIDTH);
        let mut link_iter = link_input_display.split('\n');
        assert_eq!(link_first_line, link_iter.next().unwrap());
        assert_eq!(link_second_line, link_iter.next().unwrap());
    }

    #[actix_rt::test]
    async fn test_key_events() {
        let mut repl = WashRepl::default();
        const OUTPUT_SCROLL: u16 = 42;
        const OUTPUT_CURSOR: usize = 30;
        const INPUT_HISTORY: &str = "ctl get hosts";
        const INPUT: &str =
            "ctl get inventory NBLX6IFXQGPPK74GG7Q4OVLDTXB3MPKLCXX7LPEXD4QP7DSD2HN7L56D";
        let output: Vec<String> = vec!["command output".to_string(); OUTPUT_CURSOR];

        // REPL input state setup
        repl.input_state
            .history
            .push(INPUT_HISTORY.chars().collect::<Vec<char>>());
        repl.input_state
            .history
            .push(INPUT_HISTORY.chars().collect::<Vec<char>>());
        repl.input_state.history_cursor += 2;
        assert_eq!(repl.input_state.history_cursor, 2);
        assert_eq!(repl.input_state.history.len(), 2);
        for c in INPUT.chars() {
            repl.handle_key_event(Key::Char(c)).await.unwrap();
        }
        assert_eq!(repl.input_state.input_cursor, INPUT.len());

        // REPL output state setup
        repl.output_state.lock().unwrap().output_scroll += OUTPUT_SCROLL;
        repl.output_state.lock().unwrap().output = output;
        repl.output_state.lock().unwrap().output_cursor += OUTPUT_CURSOR;
        assert_eq!(
            repl.output_state.lock().unwrap().output_scroll,
            OUTPUT_SCROLL
        );
        assert_eq!(
            repl.output_state.lock().unwrap().output_cursor,
            OUTPUT_CURSOR
        );

        // PageUp / PageDown with REPL focus
        repl.handle_key_event(Key::PageUp).await.unwrap();
        assert_eq!(
            repl.output_state.lock().unwrap().output_cursor,
            OUTPUT_CURSOR - 1
        );
        repl.handle_key_event(Key::PageUp).await.unwrap();
        assert_eq!(
            repl.output_state.lock().unwrap().output_cursor,
            OUTPUT_CURSOR - 2
        );
        repl.handle_key_event(Key::PageDown).await.unwrap();
        assert_eq!(
            repl.output_state.lock().unwrap().output_cursor,
            OUTPUT_CURSOR - 1
        );

        // Left/Right with REPL focus
        repl.handle_key_event(Key::Left).await.unwrap();
        repl.handle_key_event(Key::Left).await.unwrap();
        repl.handle_key_event(Key::Left).await.unwrap();
        assert_eq!(repl.input_state.input_cursor, INPUT.len() - 3);
        repl.handle_key_event(Key::Right).await.unwrap();
        repl.handle_key_event(Key::Right).await.unwrap();
        assert_eq!(repl.input_state.input_cursor, INPUT.len() - 1);
        repl.handle_key_event(Key::Right).await.unwrap();
        assert_eq!(repl.input_state.input_cursor, INPUT.len());

        // Backspace with REPL focus
        repl.handle_key_event(Key::Backspace).await.unwrap();
        repl.handle_key_event(Key::Backspace).await.unwrap();
        repl.handle_key_event(Key::Backspace).await.unwrap();
        repl.handle_key_event(Key::Backspace).await.unwrap();
        assert_eq!(repl.input_state.input_cursor, INPUT.len() - 4);
        assert_eq!(
            &repl.input_state.input,
            &INPUT[..INPUT.len() - 4].chars().collect::<Vec<char>>()
        );

        // ALT+Left('b') / Right('f')
        //TODO(issue #67): Ensure cursor navigates by one "word"
        assert!(repl.handle_key_event(Key::Alt('b')).await.is_ok());
        assert!(repl.handle_key_event(Key::Alt('f')).await.is_ok());

        // Up / Down with REPL focus
        repl.handle_key_event(Key::Up).await.unwrap();
        assert_eq!(repl.input_state.history_cursor, 1);
        assert_eq!(
            repl.input_state.input,
            INPUT_HISTORY.chars().collect::<Vec<char>>()
        );
        assert_eq!(repl.input_state.input_cursor, INPUT_HISTORY.len());
        repl.handle_key_event(Key::Down).await.unwrap();
        assert_eq!(repl.input_state.history_cursor, 2);
        assert!(repl.input_state.input.is_empty());
        assert_eq!(repl.input_state.input_cursor, 0);
        repl.handle_key_event(Key::Up).await.unwrap();
        repl.handle_key_event(Key::Up).await.unwrap();
        repl.handle_key_event(Key::Down).await.unwrap();
        assert_eq!(repl.input_state.history_cursor, 1);
        assert_eq!(
            repl.input_state.input,
            INPUT_HISTORY.chars().collect::<Vec<char>>()
        );
        assert_eq!(repl.input_state.input_cursor, INPUT_HISTORY.len());

        // Clear REPL input again
        repl.handle_key_event(Key::Down).await.unwrap();

        repl.handle_key_event(Key::Char('c')).await.unwrap();
        repl.handle_key_event(Key::Char('l')).await.unwrap();
        repl.handle_key_event(Key::Char('e')).await.unwrap();
        repl.handle_key_event(Key::Char('a')).await.unwrap();
        repl.handle_key_event(Key::Char('r')).await.unwrap();
        repl.handle_key_event(Key::Char('\n')).await.unwrap();

        assert_eq!(repl.input_state, InputState::default());

        let quit_options = vec!["exit", "logout", "q", ":q!"];
        for opt in quit_options {
            for c in opt.chars() {
                repl.handle_key_event(Key::Char(c)).await.unwrap();
            }
            let res = repl.handle_key_event(Key::Char('\n')).await;
            match res {
                Err(e) => assert_eq!(format!("{}", e), "REPL Quit"),
                _ => panic!("REPL exit option {} did not quit REPL", opt),
            }
        }
    }

    #[test]
    fn test_log_level_from_str() -> Result<()> {
        use std::str::FromStr;
        const ERROR: &str = "error";
        const WARN: &str = "warn";
        const DEBUG: &str = "debug";
        const INFO: &str = "info";
        const TRACE: &str = "trace";
        const FOO: &str = "foo";

        assert_eq!(LogLevel::from_str(ERROR)?, LogLevel::Error);
        assert_eq!(LogLevel::from_str(WARN)?, LogLevel::Warn);
        assert_eq!(LogLevel::from_str(DEBUG)?, LogLevel::Debug);
        assert_eq!(LogLevel::from_str(INFO)?, LogLevel::Info);
        assert_eq!(LogLevel::from_str(TRACE)?, LogLevel::Trace);
        assert_eq!(LogLevel::from_str(FOO)?, LogLevel::Trace);
        Ok(())
    }
}
