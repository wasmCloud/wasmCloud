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

waSCC CLI utilities
";

#[derive(Debug, Clone, StructOpt)]
#[structopt(name = "wcc", about = ASCII)]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
    
}

#[derive(Debug, Clone, StructOpt)]
enum CliCommand {
    /// nkeys
    #[structopt(name = "nk")]
    Nk(NkCommand),

    //TODO: Add wascap, gantry, and latticectl options

}

#[derive(Debug, Clone, StructOpt)]
struct NkCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

fn main() {
    let cli = Cli::from_args();
    println!("{:#?}", cli);
}
                                       
                                       
                                       

