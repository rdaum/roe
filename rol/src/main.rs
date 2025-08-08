mod ast;
mod bench;
mod bytecode;
mod compiler;
mod environment;
mod gc;
mod heap;
mod heap_ptr;
mod integration_tests;
mod jit;
mod lexer;
mod mmtk_binding;
mod parser;
mod protocol;
mod repl;
mod scheduler;
mod symbol;
mod var;

fn main() {
    // Set up logging to suppress verbose Cranelift output
    if std::env::var("RUST_LOG").is_err() {
        unsafe { std::env::set_var("RUST_LOG", "mmtk=info,cranelift_jit=error"); }
    }
    
    // Initialize MMTk garbage collector
    if let Err(err) = mmtk_binding::initialize_mmtk() {
        eprintln!("Failed to initialize MMTk: {err}");
        std::process::exit(1);
    }
    
    // Bind mutator for the main thread
    if let Err(err) = mmtk_binding::mmtk_bind_mutator() {
        eprintln!("Failed to bind mutator for main thread: {err}");
        std::process::exit(1);
    }
    
    if let Err(err) = repl::start_repl() {
        eprintln!("REPL error: {err}");
        std::process::exit(1);
    }
}
