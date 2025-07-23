//! Read-Eval-Print Loop for the Lisp interpreter.
//! Provides an interactive shell with readline support, history, and error handling.

use crate::compiler::Compiler;
use crate::environment::Environment;
use crate::parser::parse_expr_string;
use crate::var::Var;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

/// REPL state that maintains the compiler and global environment
pub struct Repl {
    compiler: Compiler,
    editor: DefaultEditor,
    global_env_ptr: *mut Environment,
    global_env_var: Var,
}

impl Repl {
    /// Create a new REPL instance
    pub fn new() -> std::result::Result<Self, ReadlineError> {
        let compiler = Compiler::new();
        let editor = DefaultEditor::new()?;
        
        // Create a global environment (empty for now)
        let global_env_ptr = Environment::from_values(&[], None);
        let global_env_var = Var::environment(global_env_ptr);
        
        Ok(Self {
            compiler,
            editor,
            global_env_ptr,
            global_env_var,
        })
    }
    
    /// Evaluate a Lisp expression string and return the result
    pub fn eval(&mut self, input: &str) -> std::result::Result<Var, Box<dyn std::error::Error>> {
        // Parse the expression
        let expr = parse_expr_string(input).map_err(|e| e as Box<dyn std::error::Error>)?;
        
        // Compile to machine code
        let func_ptr = self.compiler.compile_expr(&expr).map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)) as Box<dyn std::error::Error>)?;
        
        // Execute with global environment
        let func: fn(u64) -> u64 = unsafe { std::mem::transmute(func_ptr) };
        let result_bits = func(self.global_env_var.as_u64());
        
        // Return result
        Ok(Var::from_u64(result_bits))
    }
    
    /// Format a Var for display in the REPL
    fn format_result(&self, var: &Var) -> String {
        match var.get_type() {
            crate::var::VarType::I32 => {
                if let Some(n) = var.as_int() {
                    format!("{}", n)
                } else {
                    format!("{:?}", var)
                }
            }
            crate::var::VarType::F64 => {
                if let Some(n) = var.as_double() {
                    format!("{}", n)
                } else {
                    format!("{:?}", var)
                }
            }
            crate::var::VarType::String => {
                if let Some(s) = var.as_string() {
                    format!("\"{}\"", s)
                } else {
                    format!("{:?}", var)
                }
            }
            crate::var::VarType::Bool => {
                if let Some(b) = var.as_bool() {
                    if b { "true".to_string() } else { "false".to_string() }
                } else {
                    format!("{:?}", var)
                }
            }
            crate::var::VarType::List => {
                // TODO: Format lists nicely
                format!("{:?}", var)
            }
            crate::var::VarType::Environment => {
                format!("#<environment>")
            }
            crate::var::VarType::Symbol => {
                format!("{:?}", var)
            }
            crate::var::VarType::None => {
                "nil".to_string()
            }
            crate::var::VarType::Pointer => {
                format!("#<pointer>")
            }
            crate::var::VarType::Closure => {
                if let Some(closure_ptr) = var.as_closure() {
                    unsafe {
                        format!("#<closure:{}>", (*closure_ptr).arity)
                    }
                } else {
                    format!("#<closure:invalid>")
                }
            }
        }
    }
    
    /// Run the main REPL loop
    pub fn run(&mut self) -> std::result::Result<(), ReadlineError> {
        println!("Welcome to ROL - Ryan's Own Lisp!");
        println!("A JIT-compiled Lisp interpreter with lexical scoping.");
        println!("Type expressions to evaluate them, or 'quit' to exit.");
        println!();
        
        loop {
            match self.editor.readline("rol> ") {
                Ok(line) => {
                    let line = line.trim();
                    
                    // Handle special commands
                    if line.is_empty() {
                        continue;
                    }
                    
                    if line == "quit" || line == "exit" || line == ":q" {
                        println!("Goodbye!");
                        break;
                    }
                    
                    if line == "help" || line == ":help" {
                        self.print_help();
                        continue;
                    }
                    
                    if line == ":fib" {
                        println!("Creating fibonacci function...");
                        println!("For now, fibonacci must be implemented as lambda expressions.");
                        println!("Try this when lambda support is complete:");
                        println!("  (let ((fib (lambda (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))))) (fib 10))");
                        continue;
                    }
                    
                    // Add to history
                    self.editor.add_history_entry(line)?;
                    
                    // Evaluate the expression
                    match self.eval(line) {
                        Ok(result) => {
                            println!("{}", self.format_result(&result));
                        }
                        Err(err) => {
                            println!("Error: {}", err);
                        }
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("^D");
                    break;
                }
                Err(err) => {
                    println!("Error: {:?}", err);
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    /// Print help information
    fn print_help(&self) {
        println!("ROL - Ryan's Own Lisp Help");
        println!("==========================");
        println!();
        println!("Basic Syntax:");
        println!("  42                    ; integers");
        println!("  3.14                  ; floats");
        println!("  \"hello\"               ; strings");
        println!("  :keyword              ; keywords");
        println!();
        println!("Arithmetic:");
        println!("  (+ 2 3)               ; addition → 5");
        println!("  (+ 2.5 3)             ; mixed types → 5.5 (float)");
        println!("  (+ (+ 1 2) 3)         ; nested → 6");
        println!();
        println!("Variables:");
        println!("  (let ((x 5)) x)       ; let binding → 5");
        println!("  (let ((x 5)) (+ x 2)) ; using variables → 7");
        println!("  (let ((x 2) (y 3)) (+ x y))  ; multiple bindings → 5");
        println!();
        println!("Conditionals:");
        println!("  (if 1 42 24)          ; truthy condition → 42");
        println!("  (if 0 42 24)          ; falsy condition → 24");
        println!("  (if (+ 1 2) \"yes\" \"no\")  ; with expressions → \"yes\"");
        println!();
        println!("Nested Expressions:");
        println!("  (let ((x 3)) (let ((y 4)) (+ x y)))  ; nested scoping → 7");
        println!("  (+ (let ((x 1) (y 2)) (+ x y)) (let ((a 3) (b 4)) (+ a b)))");
        println!();
        println!("Commands:");
        println!("  help, :help           ; show this help");
        println!("  quit, exit, :q        ; exit the REPL");
        println!("  Ctrl+C                ; interrupt current input");
        println!("  Ctrl+D                ; exit the REPL");
        println!();
        println!("Features:");
        println!("  • JIT compilation to native machine code");
        println!("  • Smart type coercion (int + int = int, mixed → float)");
        println!("  • Lexical scoping with environment chains");
        println!("  • Proper Lisp truthiness (0, 0.0, false are falsy)");
        println!("  • Readline support with history and line editing");
        println!();
    }
}

impl Drop for Repl {
    fn drop(&mut self) {
        // Clean up the global environment
        unsafe {
            Environment::free(self.global_env_ptr);
        }
    }
}

/// Create and run a REPL
pub fn start_repl() -> std::result::Result<(), ReadlineError> {
    let mut repl = Repl::new()?;
    repl.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_repl_creation() {
        let repl = Repl::new();
        assert!(repl.is_ok());
    }
    
    #[test]
    fn test_repl_eval() {
        let mut repl = Repl::new().unwrap();
        
        // Test basic arithmetic
        let result = repl.eval("(+ 2 3)").unwrap();
        assert_eq!(result.as_int(), Some(5));
        
        // Test let binding
        let result = repl.eval("(let ((x 10)) (+ x 5))").unwrap();
        assert_eq!(result.as_int(), Some(15));
        
        // Test if expression
        let result = repl.eval("(if 1 42 0)").unwrap();
        assert_eq!(result.as_int(), Some(42));
    }
    
    #[test]
    fn test_result_formatting() {
        let repl = Repl::new().unwrap();
        
        assert_eq!(repl.format_result(&Var::int(42)), "42");
        assert_eq!(repl.format_result(&Var::float(3.14)), "3.14");
        assert_eq!(repl.format_result(&Var::string("hello")), "\"hello\"");
        assert_eq!(repl.format_result(&Var::bool(true)), "true");
        assert_eq!(repl.format_result(&Var::bool(false)), "false");
        assert_eq!(repl.format_result(&Var::none()), "nil");
    }
}