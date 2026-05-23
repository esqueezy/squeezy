mod accuracy;
mod cli;
mod harness;
mod mixed;
mod oracles;
mod report;
mod runner;
mod summary;

use squeezy_core::Result;

fn main() -> Result<()> {
    runner::main()
}
