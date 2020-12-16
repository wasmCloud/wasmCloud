use crate::ctl::*;
use crate::util::{convert_error, Result};
use crossterm::event::{poll, read, DisableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use log::{debug, error, info, LevelFilter};
use std::io::{self, Stdout};
use std::{cell::RefCell, io::Write, rc::Rc};
use structopt::{clap::AppSettings, StructOpt};
use tui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use tui_logger::*;
use wasmcloud_host::HostBuilder;

const WASH_LOG_INFO: &str = "WASH_LOG";
const WASH_CMD_INFO: &str = "WASH_CMD";
const CTL_NS: &str = "default";
const WASH_PROMPT: &str = "wash> ";

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "up")]
pub struct UpCli {
    #[structopt(flatten)]
    command: UpCommand,
}

#[derive(StructOpt, Debug, Clone)]
struct UpCommand {
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
}

pub(crate) async fn handle_command(cli: UpCli) -> Result<()> {
    match cli.command {
        UpCommand { .. } => handle_up(cli.command).await,
    }
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
    /// Query lattice for information
    #[structopt(name = "get")]
    Get(GetCommand),

    /// Invokes an operation on an actor
    #[structopt(name = "call")]
    Call(CallCommand),

    /// Links an actor and a capability provider
    #[structopt(name = "link")]
    Link(LinkCommand),

    /// Starts an actor or capability provider
    #[structopt(name = "start")]
    Start(StartCommand),

    /// Starts an actor or capability provider
    #[structopt(name = "stop")]
    Stop(StopCommand),

    /// Terminates the REPL environment (also accepts 'exit', 'logout', 'q' and ':q!')
    #[structopt(name = "quit", aliases = &["exit", "logout", "q", ":q!"])]
    Quit,

    /// Clears the REPL input history
    #[structopt(name = "clear")]
    Clear,
}

#[derive(Debug, Clone)]
struct InputState {
    history: Vec<Vec<char>>,
    history_cursor: usize,
    input: Vec<char>,
    input_cursor: usize,
    multiline_history: u16, // amount to offset cursor for multiline inputs
    input_width: usize,
}

impl Default for InputState {
    fn default() -> Self {
        InputState {
            history: vec![],
            history_cursor: 0,
            input: vec![],
            input_cursor: 0,
            multiline_history: 0,
            input_width: 0,
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
    output_state: OutputState,
    tui_dispatcher: Rc<RefCell<Dispatcher<Event>>>,
    tui_state: TuiWidgetState,
}

impl Default for WashRepl {
    fn default() -> Self {
        WashRepl {
            input_state: InputState::default(),
            output_state: OutputState::default(),
            tui_dispatcher: Rc::new(RefCell::new(Dispatcher::<Event>::new())),
            tui_state: TuiWidgetState::new(),
        }
    }
}

impl WashRepl {
    /// Using the state of the REPL, display information in the terminal window
    fn draw_ui(
        &mut self,
        terminal: &mut Terminal<tui::backend::CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
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
            draw_output_panel(frame, &mut self.output_state, io_chunks[1]);
            draw_smart_logger(frame, main_chunks[1], &self.tui_state, &self.tui_dispatcher);
        })?;
        Ok(())
    }

    /// Handles key input by the user into the REPL
    async fn handle_key(&mut self, code: KeyCode, modifier: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Char(c) => {
                self.input_state
                    .input
                    .insert(self.input_state.input_cursor, c);
                self.input_state.input_cursor += 1;
            }
            KeyCode::Left => {
                if self.input_state.input_cursor > 0 {
                    self.input_state.input_cursor -= 1
                }
            }
            KeyCode::Right => {
                if self.input_state.input_cursor < self.input_state.input.len() {
                    self.input_state.input_cursor += 1
                }
            }
            KeyCode::Up => {
                if modifier == KeyModifiers::SHIFT
                    && self.output_state.output_cursor > 0
                    && self.output_state.output_scroll > 0
                {
                    self.output_state.output_cursor -= 1;
                } else if self.input_state.history_cursor > 0 && modifier == KeyModifiers::NONE {
                    self.input_state.history_cursor -= 1;
                    self.input_state.input =
                        self.input_state.history[self.input_state.history_cursor].clone();
                    self.input_state.input_cursor = self.input_state.input.len();
                }
            }
            KeyCode::Down => {
                if modifier == KeyModifiers::SHIFT
                    && self.output_state.output_cursor < self.output_state.output.len()
                {
                    self.output_state.output_cursor += 1;
                } else if modifier == KeyModifiers::NONE {
                    if self.input_state.history.is_empty() {
                        return Ok(());
                    };
                    if self.input_state.history_cursor < self.input_state.history.len() - 1
                        && self.input_state.history_cursor > 0
                    {
                        self.input_state.history_cursor += 1;
                        self.input_state.input =
                            self.input_state.history[self.input_state.history_cursor].clone();
                        self.input_state.input_cursor = self.input_state.input.len();
                    } else if self.input_state.history_cursor >= self.input_state.history.len() - 1
                    {
                        self.input_state.history_cursor = self.input_state.history.len();
                        self.input_state.input.clear();
                        self.input_state.input_cursor = 0;
                    }
                }
            }
            KeyCode::Backspace => {
                if self.input_state.input_cursor > 0
                    && self.input_state.input_cursor <= self.input_state.input.len()
                {
                    self.input_state.input_cursor -= 1;
                    self.input_state.input.remove(self.input_state.input_cursor);
                };
            }
            KeyCode::Enter => {
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
                        //TODO(brooksmtownsend): Add loading / fetching messages to longer / blocking calls
                        match cmd {
                            Clear => {
                                info!(target: WASH_LOG_INFO, "Clearing REPL history");
                                self.input_state = InputState::default();
                            }
                            Quit => {
                                info!(target: WASH_CMD_INFO, "Goodbye");
                                return Err("REPL Quit".into());
                            }
                            Call(callcmd) => {
                                match handle_call(callcmd, &mut self.output_state).await {
                                    Ok(r) => r,
                                    Err(e) => error!("Error handling call: {}", e),
                                };
                            }
                            Get(getcmd) => {
                                match handle_get(getcmd, &mut self.output_state).await {
                                    Ok(r) => r,
                                    Err(e) => error!("Error handling get: {}", e),
                                };
                            }
                            Link(linkcmd) => {
                                match handle_link(linkcmd, &mut self.output_state).await {
                                    Ok(r) => r,
                                    Err(e) => error!("Error handling link: {}", e),
                                }
                            }
                            Start(startcmd) => {
                                match handle_start(startcmd, &mut self.output_state).await {
                                    Ok(r) => r,
                                    Err(e) => error!("Error handling start: {}", e),
                                };
                            }
                            Stop(stopcmd) => {
                                match handle_stop(stopcmd, &mut self.output_state).await {
                                    Ok(r) => r,
                                    Err(e) => error!("Error handling stop: {}", e),
                                };
                            }
                        }
                    }
                    Err(e) => {
                        use structopt::clap::ErrorKind::*;
                        // HelpDisplayed is the StructOpt help text error, which should be displayed as info
                        match e.kind {
                            HelpDisplayed => info!(target: WASH_CMD_INFO, "\n{}", e.message),
                            _ => error!(target: WASH_CMD_INFO, "\n{}", e.message),
                        }
                    }
                };
            }
            _ => (),
        };
        Ok(())
    }
}

/// Launches REPL environment
async fn handle_up(cmd: UpCommand) -> Result<()> {
    // Initialize logger at default level Trace
    init_logger(LevelFilter::Trace).unwrap();
    set_default_level(LevelFilter::Trace);

    // Initialize terminal
    let backend = {
        crossterm::terminal::enable_raw_mode().unwrap();
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen).unwrap();
        CrosstermBackend::new(stdout)
    };
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.clear().unwrap();
    terminal.hide_cursor().unwrap();

    // Start REPL
    let mut repl = WashRepl::default();
    repl.draw_ui(&mut terminal)?;
    info!(target: WASH_LOG_INFO, "Initializing REPL...");
    // Sending SPACE event to tui logger to hide disabled logs
    let evt = Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    repl.tui_dispatcher.borrow_mut().dispatch(&evt);
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
                            "Error connecting to NATS at {}:{}, running in hostless mode",
                            cmd.rpc_host,
                            cmd.rpc_port
                        );
                        return;
                    }
                };
            let nc_control =
                match nats::asynk::connect(&format!("{}:{}", cmd.rpc_host, cmd.rpc_port)).await {
                    Ok(conn) => conn,
                    Err(_e) => {
                        error!(
                            target: WASH_CMD_INFO,
                            "Error connecting to NATS at {}:{}, running in hostless mode",
                            cmd.rpc_host,
                            cmd.rpc_port
                        );
                        return;
                    }
                };
            let host = HostBuilder::new()
                .with_namespace(CTL_NS)
                .with_rpc_client(nc_rpc)
                .with_control_client(nc_control)
                .with_label("repl_mode", "true")
                .oci_allow_latest()
                .oci_allow_insecure()
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
    let mut repl_focus = true;
    loop {
        // Polling here results in a nonblocking wait for events
        if poll(std::time::Duration::from_millis(50))? {
            let res = match read()? {
                // Tab toggles input focus between REPL and Tui logger selector
                Event::Key(KeyEvent {
                    code: KeyCode::Tab, ..
                }) => {
                    repl_focus = !repl_focus;
                    info!(
                        target: WASH_CMD_INFO,
                        "Switched command focus to {}",
                        if repl_focus {
                            "REPL"
                        } else {
                            "Logger selector"
                        }
                    );
                    Ok(())
                }
                // Dispatch events for REPL interpretation
                Event::Key(KeyEvent { code, modifiers }) if repl_focus => {
                    repl.handle_key(code, modifiers).await
                }
                // Dispatch events for Tui Target interpretation
                evt => {
                    repl.tui_dispatcher.borrow_mut().dispatch(&evt);
                    Ok(())
                }
            };
            repl.draw_ui(&mut terminal)?;

            // Exit the terminal gracefully
            if res.is_err() {
                cleanup_terminal(&mut terminal);
                break;
            }
        } else {
            // If no events occur, draw UI to show asynchronous logs
            repl.draw_ui(&mut terminal)?;
        }
    }
    cleanup_terminal(&mut terminal);
    Ok(())
}

async fn handle_get(get_cmd: GetCommand, output_state: &mut OutputState) -> Result<()> {
    match get_cmd {
        GetCommand::Claims(cmd) => {
            let claims_list = get_claims(cmd).await?;
            debug!(target: WASH_CMD_INFO, "\n{:?}", claims_list);
            log_to_output(
                output_state,
                claims_table(claims_list, Some(output_state.output_width)),
            )
        }
        GetCommand::Hosts(cmd) => {
            let hosts = get_hosts(cmd).await?;
            debug!(target: WASH_CMD_INFO, "\n{:?}", hosts);
            log_to_output(
                output_state,
                hosts_table(hosts, Some(output_state.output_width)),
            )
        }
        GetCommand::HostInventory(cmd) => {
            let inventory = get_host_inventory(cmd).await?;
            debug!(target: WASH_CMD_INFO, "\n{:?}", inventory);
            log_to_output(
                output_state,
                host_inventory_table(inventory, Some(output_state.output_width)),
            )
        }
    };
    Ok(())
}

async fn handle_start(start_cmd: StartCommand, output_state: &mut OutputState) -> Result<()> {
    match start_cmd {
        StartCommand::Actor(cmd) => {
            info!(
                target: WASH_CMD_INFO,
                "Sending request to start actor {}", cmd.actor_ref
            );
            match start_actor(cmd).await {
                Ok(ack) => {
                    debug!(target: WASH_CMD_INFO, "Start actor ack: {:?}", ack);
                    log_to_output(
                        output_state,
                        format!("Starting {} ({})", ack.actor_ref, ack.actor_id),
                    );
                }
                Err(e) => error!(target: WASH_CMD_INFO, "{}", e),
            };
        }
        StartCommand::Provider(cmd) => {
            info!(
                target: WASH_CMD_INFO,
                "Sending request to start provider {}", cmd.provider_ref
            );
            match start_provider(cmd).await {
                Ok(ack) => {
                    debug!(target: WASH_CMD_INFO, "Start provider ack: {:?}", ack);
                    log_to_output(
                        output_state,
                        format!("Starting {} ({})", ack.provider_ref, ack.provider_id),
                    );
                }
                Err(e) => error!(target: WASH_CMD_INFO, "{}", e),
            };
        }
    };
    Ok(())
}

async fn handle_stop(stop_cmd: StopCommand, output_state: &mut OutputState) -> Result<()> {
    match stop_cmd {
        StopCommand::Actor(cmd) => {
            let actor_ref = cmd.actor_ref.clone();
            match stop_actor(cmd).await {
                Ok(ack) => {
                    debug!(target: WASH_CMD_INFO, "Stop actor ack: {:?}", ack);
                    if let Some(err) = ack.failure {
                        error!(target: WASH_CMD_INFO, "{}", err);
                    } else {
                        log_to_output(
                            output_state,
                            format!("Successfully stopped actor {}", actor_ref),
                        );
                    }
                }
                Err(e) => error!(target: WASH_CMD_INFO, "{}", e),
            };
        }
        StopCommand::Provider(cmd) => {
            let provider_ref = cmd.provider_ref.clone();
            match stop_provider(cmd).await {
                Ok(ack) => {
                    debug!(target: WASH_CMD_INFO, "Stop provider ack: {:?}", ack);
                    if let Some(err) = ack.failure {
                        error!(target: WASH_CMD_INFO, "{}", err);
                    } else {
                        log_to_output(
                            output_state,
                            format!("Successfully stopped provider {}", provider_ref),
                        );
                    }
                }
                Err(e) => error!(target: WASH_CMD_INFO, "{}", e),
            };
        }
    };
    Ok(())
}

async fn handle_link(cmd: LinkCommand, output_state: &mut OutputState) -> Result<()> {
    match advertise_link(cmd.clone()).await {
        Ok(_r) => {
            info!(target: WASH_CMD_INFO, "Published link successfully");
            log_to_output(
                output_state,
                format!("Published link {} <-> {}", cmd.actor_id, cmd.provider_id),
            );
        }
        Err(e) => error!(target: WASH_CMD_INFO, "Error publishing link {}", e),
    }
    Ok(())
}

async fn handle_call(cmd: CallCommand, output_state: &mut OutputState) -> Result<()> {
    match call_actor(cmd).await {
        Ok(r) => match r.error {
            Some(e) => error!(target: WASH_CMD_INFO, "Error invoking actor: {}", e),
            None => {
                debug!(
                    target: WASH_CMD_INFO,
                    "Invocation successful ({})", r.invocation_id
                );
                //TODO: String::from_utf8_lossy should be decoder only if one is not available
                let out = String::from_utf8_lossy(&r.msg);
                log_to_output(
                    output_state,
                    format!("Call response (raw): {}", out.to_string()),
                );
            }
        },
        Err(e) => error!(target: WASH_CMD_INFO, "unsuccessful call: {:?}", e),
    };
    Ok(())
}

/// Helper function to exit the alternate tui terminal without corrupting the user terminal
fn cleanup_terminal(terminal: &mut Terminal<tui::backend::CrosstermBackend<std::io::Stdout>>) {
    terminal.show_cursor().unwrap();
    terminal.clear().unwrap();
    crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture).unwrap();
    terminal::disable_raw_mode().unwrap();
}

/// Append a message to the output log
fn log_to_output(state: &mut OutputState, out: String) {
    // Reset output scroll to bottom
    state.output_cursor = state.output.len();

    // Newlines are used here for accurate scrolling in the Output pane
    out.split('\n').for_each(|line| {
        state.output.push(line.to_string());
        state.output_cursor += 1;
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
fn draw_input_panel(
    frame: &mut Frame<CrosstermBackend<Stdout>>,
    state: &mut InputState,
    chunk: Rect,
) {
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

    // Draw REPL panel
    let input_panel = Paragraph::new(display)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " REPL ",
            Style::default().add_modifier(Modifier::BOLD),
        )))
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
    frame: &mut Frame<CrosstermBackend<Stdout>>,
    state: &mut OutputState,
    chunk: Rect,
) {
    let output_logs: String = state.output.iter().map(|h| format!("{}\n", h)).collect();

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
    state.output_width = chunk.width as usize;

    // Draw REPL panel
    let output_panel = Paragraph::new(output_logs)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " OUTPUT (SHIFT+UP/DOWN to scroll) ",
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Left)
        .scroll((state.output_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(output_panel, chunk);
}

/// Draws the Tui smart logger widget in the provided frame
fn draw_smart_logger(
    frame: &mut Frame<CrosstermBackend<Stdout>>,
    chunk: Rect,
    state: &TuiWidgetState,
    dispatcher: &Rc<RefCell<Dispatcher<Event>>>,
) {
    dispatcher.borrow_mut().clear();
    let selector_panel = TuiLoggerSmartWidget::default()
        .style_error(Style::default().fg(Color::Red))
        .style_debug(Style::default().fg(Color::Green))
        .style_warn(Style::default().fg(Color::Yellow))
        .style_trace(Style::default().fg(Color::Magenta))
        .style_info(Style::default().fg(Color::Cyan))
        .state(state)
        .dispatcher(dispatcher.clone());
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
