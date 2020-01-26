use structopt::StructOpt;

#[derive(StructOpt,Debug)]
enum Command {
    /// Dummy command
    Dummy,
}

#[derive(StructOpt,Debug)]
struct Opt {
    #[structopt(subcommand)]
    cmd: Command,
}

fn main() {
    let opt = Opt::from_args();
    match opt.cmd {
        Command::Dummy => {
            println!("Dummy");
        }
    }
}
