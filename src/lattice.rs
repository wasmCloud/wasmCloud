extern crate latticeclient;

// use latticeclient::*;
use std::error::Error;
use std::{collections::HashMap, path::PathBuf, time::Duration};

use crossbeam::unbounded;
use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(Debug, StructOpt, Clone)]
#[structopt(
global_settings(& [AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands, AppSettings::GlobalVersion]),
name = "lattice",
about = "A command line utility for interacting with a waSCC lattice")]
/// latticectl interacts with a waSCC lattice the same way the waSCC host would, and uses the same
/// environment variables to connect by default
pub struct LatticeCli {
    #[structopt(flatten)]
    pub command: LatticeCliCommand,

    /// The host IP of the nearest NATS server/leaf node to connect to the lattice
    #[structopt(
        short,
        long,
        env = "LATTICE_HOST",
        hide_env_values = true,
        default_value = "127.0.0.1"
    )]
    pub url: String,

    /// Credentials file used to authenticate against NATS
    #[structopt(
        short,
        long,
        env = "LATTICE_CREDS_FILE",
        hide_env_values = true,
        parse(from_os_str)
    )]
    pub creds: Option<PathBuf>,

    /// Lattice invocation / request timeout period, in milliseconds
    #[structopt(
        short = "t",
        long = "timeout",
        env = "LATTICE_RPC_TIMEOUT_MILLIS",
        hide_env_values = true,
        default_value = "600"
    )]
    pub call_timeout: u64,

    /// Lattice namespace
    #[structopt(
        short = "n",
        long = "namespace",
        env = "LATTICE_NAMESPACE",
        hide_env_values = true
    )]
    pub namespace: Option<String>,
    /// Render the output in JSON (if the command supports it)
    #[structopt(short, long)]
    pub json: bool,
}

#[derive(Debug, Clone, StructOpt)]
pub enum LatticeCliCommand {
    /// List entities of various types within the lattice
    #[structopt(name = "list")]
    List {
        /// The entity type to list (actors, bindings, capabilities(caps), hosts)
        entity_type: String,
    },
    #[structopt(name = "watch")]
    /// Watch events on the lattice
    Watch,
    #[structopt(name = "start")]
    /// Hold a lattice auction for a given actor and start it if a suitable host is found
    Start {
        /// An OCI image reference of the actor to be launched
        actor_ref: String,
        /// Add limiting constraints to filter potential target hosts (in the form of label=value)
        #[structopt(short = "c", parse(try_from_str = parse_key_val), number_of_values = 1)]
        constraint: Vec<(String, String)>,
    },
    /// Tell a given host to terminate the given actor
    #[structopt(name = "stop")]
    Stop { actor: String, host_id: String },
}

//         match handle_command(
//             cmd,
//             args.url,
//             args.json,
//             args.creds,
//             args.namespace,
//             Duration::from_millis(args.call_timeout),
//         ) {
//             Ok(_) => 0,
//             Err(e) => {
//                 eprintln!("Latticectl Error: {}", e);
//                 1
//             }
//         },
//     )
// }

pub fn handle_command(
    cli: LatticeCli,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let cmd = cli.command;
    let url = cli.url;
    let json = cli.json;
    let creds = cli.creds;
    let namespace = cli.namespace;
    let timeout = Duration::from_millis(cli.call_timeout);
    match cmd {
        LatticeCliCommand::List { entity_type } => {
            list_entities(&entity_type, &url, creds, timeout, json, namespace)
        }
        LatticeCliCommand::Watch => watch_events(&url, creds, timeout, json, namespace),
        LatticeCliCommand::Start {
            actor_ref,
            constraint,
        } => start_actor(&url, creds, timeout, json, namespace, actor_ref, constraint),
        LatticeCliCommand::Stop { actor, host_id } => {
            stop_actor(&url, creds, timeout, json, namespace, actor, host_id)
        }
    }
}

fn start_actor(
    url: &str,
    creds: Option<PathBuf>,
    timeout: Duration,
    json: bool,
    namespace: Option<String>,
    actor: String,
    constraints: Vec<(String, String)>,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let client = latticeclient::Client::new(url, creds, timeout, namespace);
    let candidates =
        client.perform_actor_launch_auction(&actor, constraints_to_hashmap(constraints))?;
    if candidates.len() > 0 {
        let ack = client.launch_actor_on_host(&actor, &candidates[0].host_id)?;
        if ack.actor_id != actor || ack.host != candidates[0].host_id {
            return Err(format!("Received unexpected acknowledgement: {:?}", ack).into());
        }
        if json {
            println!("{}", serde_json::to_string(&ack)?);
        } else {
            println!(
                "Host {} acknowledged request to launch actor {}.",
                ack.host, ack.actor_id
            );
        }
    } else {
        println!("Did not receive a response to the actor schedule auction.");
    }
    Ok(())
}

fn stop_actor(
    url: &str,
    creds: Option<PathBuf>,
    timeout: Duration,
    _json: bool,
    namespace: Option<String>,
    actor: String,
    host_id: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = latticeclient::Client::new(url, creds, timeout, namespace);
    client.stop_actor_on_host(&actor, &host_id)?;
    println!("Termination command sent.");
    Ok(())
}

fn watch_events(
    url: &str,
    creds: Option<PathBuf>,
    timeout: Duration,
    json: bool,
    namespace: Option<String>,
) -> Result<(), Box<dyn ::std::error::Error>> {
    if !json {
        println!("Watching lattice events, Ctrl+C to abort...");
    }
    let client = latticeclient::Client::new(url, creds, timeout, namespace);
    let (s, r) = unbounded();
    client.watch_events(s)?;
    loop {
        let be = r.recv()?;
        if json {
            let raw = serde_json::to_string(&be)?;
            println!("{}", raw);
        } else {
            println!("{}", be);
        }
    }
}

fn list_entities(
    entity_type: &str,
    url: &str,
    creds: Option<PathBuf>,
    timeout: Duration,
    json: bool,
    namespace: Option<String>,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let client = latticeclient::Client::new(url, creds, timeout, namespace);
    match entity_type.to_lowercase().trim() {
        "hosts" => render_hosts(&client, json),
        "actors" => render_actors(&client, json),
        "bindings" => render_bindings(&client, json),
        "capabilities" | "caps" => render_capabilities(&client, json),
        _ => Err(
            "Unknown entity type. Valid types are: hosts, actors, capabilities, bindings".into(),
        ),
    }
}

fn render_actors(
    client: &latticeclient::Client,
    json: bool,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let actors = client.get_actors()?;
    if json {
        println!("{}", serde_json::to_string(&actors)?);
    } else {
        for (host, actors) in actors {
            println!("\nHost {}:", host);
            for actor in actors {
                let md = actor.metadata.clone().unwrap();
                println!(
                    "\t{} - {}  v{} ({})",
                    actor.subject,
                    actor.name(),
                    md.ver.unwrap_or("???".into()),
                    md.rev.unwrap_or(0)
                );
            }
        }
    }
    Ok(())
}

fn render_hosts(
    client: &latticeclient::Client,
    json: bool,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let hosts = client.get_hosts()?;
    if json {
        println!("{}", serde_json::to_string(&hosts)?);
    } else {
        for host in hosts {
            println!(
                "[{}] Uptime {}s, Labels: {}",
                host.id,
                host.uptime_ms / 1000,
                host.labels
                    .keys()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    Ok(())
}

fn render_capabilities(
    client: &latticeclient::Client,
    json: bool,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let caps = client.get_capabilities()?;
    if json {
        println!("{}", serde_json::to_string(&caps)?);
    } else {
        for (host, caps) in caps {
            println!("{}", host);
            for cap in caps {
                println!(
                    "\t{},{} - Total Operations {}",
                    cap.descriptor.id,
                    cap.binding_name,
                    cap.descriptor.supported_operations.len()
                );
            }
        }
    }
    Ok(())
}

fn render_bindings(
    client: &latticeclient::Client,
    json: bool,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let bindings = client.get_bindings()?;
    if json {
        println!("{}", serde_json::to_string(&bindings)?);
    } else {
        for (host, bindings) in bindings {
            println!("Host {}", host);
            for binding in bindings {
                println!(
                    "\t{} -> {},{} - {} values",
                    binding.actor,
                    binding.capability_id,
                    binding.binding_name,
                    binding.configuration.len()
                );
            }
        }
    }
    Ok(())
}

/// Parse a single key-value pair
fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: Error + 'static,
    U: std::str::FromStr,
    U::Err: Error + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

fn constraints_to_hashmap(input: Vec<(String, String)>) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    for (k, v) in input {
        hm.insert(k.to_owned(), v.to_owned());
    }
    hm
}