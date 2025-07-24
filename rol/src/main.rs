mod ast;
mod bytecode;
mod compiler;
mod environment;
mod gc;
mod heap;
mod integration_tests;
mod jit;
mod lexer;
mod parser;
mod protocol;
mod repl;
mod symbol;
mod var;

fn main() {
    if let Err(err) = repl::start_repl() {
        eprintln!("REPL error: {}", err);
        std::process::exit(1);
    }
}
