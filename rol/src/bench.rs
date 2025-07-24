//! Performance benchmarks for ROL Lisp interpreter.
//! Compares JIT-compiled ROL performance against native Rust implementations.

use crate::repl::Repl;
use crate::bytecode;
use std::time::Instant;

/// Native Rust recursive Fibonacci implementation
fn fib_rust(n: i32) -> i32 {
    if n < 2 {
        n
    } else {
        fib_rust(n - 1) + fib_rust(n - 2)
    }
}

/// Run Fibonacci benchmark comparing ROL vs Rust
fn run_fibonacci_benchmark() -> Result<(), Box<dyn std::error::Error>> {
    println!("ROL Fibonacci Benchmark");
    println!("======================");
    println!();
    
    // Test with Fibonacci of 25 for profiling - higher number for longer execution
    let n = 25; // Higher number for profiling
    println!("Computing Fibonacci of {n}");
    println!();
    
    // ROL Implementation
    println!("ROL (JIT-compiled Lisp):");
    println!("-----------------------");
    
    let mut repl = Repl::new()?;
    
    // Now that if expressions work correctly, we can use proper recursive Fibonacci
    let fib_def = "(defn fib [n] (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))";
    
    println!("  Compiling recursive function...");
    
    // Measure compilation time
    let compile_start = Instant::now();
    repl.eval(fib_def)?;
    let compile_time = compile_start.elapsed();
    
    println!("  Compilation time: {compile_time:?}");
    
    // Reset call counter before execution
    bytecode::get_and_reset_call_count();
    
    // Measure execution time
    let exec_start = Instant::now();
    let rol_result = repl.eval(&format!("(fib {n})"))?;
    let exec_time = exec_start.elapsed();
    
    // Get the number of function calls made
    let call_count = bytecode::get_and_reset_call_count();
    
    println!("  Debug: ROL result type: {:?}", rol_result.get_type());
    println!("  Debug: ROL result value: {rol_result:?}");
    
    let rol_value = match rol_result.as_int() {
        Some(val) => val,
        None => {
            // Try as double and convert
            if let Some(val) = rol_result.as_double() {
                val as i32
            } else {
                return Err(format!("Expected numeric result, got: {rol_result:?}").into());
            }
        }
    };
    
    println!("  Execution time:   {exec_time:?}");
    println!("  Function calls:   {call_count}");
    println!("  Result:           {rol_value}");
    println!("  Total time:       {:?}", compile_time + exec_time);
    println!();
    
    // Native Rust Implementation
    println!("Native Rust:");
    println!("------------");
    
    let rust_start = Instant::now();
    let rust_result = fib_rust(n);
    let rust_time = rust_start.elapsed();
    
    println!("  Execution time:   {rust_time:?}");
    println!("  Result:           {rust_result}");
    println!();
    
    // Verification
    if rol_value != rust_result {
        return Err(format!("Result mismatch: ROL={rol_value}, Rust={rust_result}").into());
    }
    
    // Performance comparison
    println!("Performance Analysis:");
    println!("--------------------");
    
    let total_rol_time = compile_time + exec_time;
    let slowdown_total = total_rol_time.as_secs_f64() / rust_time.as_secs_f64();
    let slowdown_exec = exec_time.as_secs_f64() / rust_time.as_secs_f64();
    
    println!("  ROL vs Rust (total):     {slowdown_total:.2}x slower");
    println!("  ROL vs Rust (exec only): {slowdown_exec:.2}x slower");
    println!("  Compilation overhead:    {:.2}% of total", 
             (compile_time.as_secs_f64() / total_rol_time.as_secs_f64()) * 100.0);
    println!("  Avg time per call:       {:.2}µs", 
             (exec_time.as_secs_f64() * 1_000_000.0) / call_count as f64);
    
    if slowdown_exec < 10.0 {
        println!("  ✓ Good JIT performance - within 10x of native Rust");
    } else if slowdown_exec < 50.0 {
        println!("  ⚠ Moderate JIT performance - 10-50x slower than native");
    } else {
        println!("  ✗ Poor JIT performance - over 50x slower than native");
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_rust_fibonacci() {
        assert_eq!(fib_rust(0), 0);
        assert_eq!(fib_rust(1), 1);
        assert_eq!(fib_rust(2), 1);
        assert_eq!(fib_rust(3), 2);
        assert_eq!(fib_rust(4), 3);
        assert_eq!(fib_rust(5), 5);
        assert_eq!(fib_rust(10), 55);
    }
    
    #[test]
    fn test_fibonacci_benchmark_small() {
        // Test with simple function since recursion needs debugging
        let mut repl = Repl::new().unwrap();
        
        // Test simple arithmetic function first
        repl.eval("(defn add_one [n] (+ n 1))").unwrap();
        let result = repl.eval("(add_one 10)").unwrap();
        
        println!("Debug: result type: {:?}", result.get_type());
        println!("Debug: result value: {result:?}");
        
        // Handle both int and double results
        let add_result = match result.as_int() {
            Some(val) => val,
            None => {
                if let Some(val) = result.as_double() {
                    val as i32
                } else {
                    panic!("Expected numeric result, got: {result:?}");
                }
            }
        };
        
        assert_eq!(add_result, 11);
        
        // Test if the proper recursive fibonacci now works with if expressions
        repl.eval("(defn fib [n] (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))").unwrap(); 
        let fib_result = repl.eval("(fib 10)").unwrap();
        
        assert_eq!(fib_result.as_int().unwrap(), 55);
        assert_eq!(fib_rust(10), 55);
    }
    
    #[test]
    fn test_fibonacci_benchmark_full() {
        run_fibonacci_benchmark().unwrap();
    }
}