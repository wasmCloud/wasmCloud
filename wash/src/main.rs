use std::path::PathBuf;
use structopt::StructOpt;

const ASCII: &str = "
 ___       __   ________  ________     
|\\  \\     |\\  \\|\\   ____\\|\\   ____\\    
\\ \\  \\    \\ \\  \\ \\  \\___|\\ \\  \\___|    
 \\ \\  \\  __\\ \\  \\ \\  \\    \\ \\  \\       
  \\ \\  \\|\\__\\_\\  \\ \\  \\____\\ \\  \\____  
   \\ \\____________\\ \\_______\\ \\_______\\
    \\|____________|\\|_______|\\|_______|

A single CLI to handle all of your waSCC tooling needs
";

#[derive(Debug, Clone, StructOpt)]
#[structopt(name = "wcc", about = ASCII)]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
}

#[derive(Debug, Clone, StructOpt)]
enum CliCommand {
    /// caps
    #[structopt(name = "caps")]
    Caps(CapsCommand),
    /// gantry
    #[structopt(name = "gantry")]
    Gantry(GantryCommand),
    /// keys
    #[structopt(name = "keys")]
    Keys(KeysCommand),
    /// lattice
    #[structopt(name = "lattice")]
    Lattice(LatticeCommand),
    /// sign
    #[structopt(name = "sign")]
    Sign(SignCommand),
}

#[derive(Debug, Clone, StructOpt)]
struct CapsCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct GantryCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct KeysCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct LatticeCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct SignCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

fn main() {
    let cli = Cli::from_args();
    println!("{:#?}", cli);
}
