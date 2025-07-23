//! AST to Cranelift IR compiler.
//! Converts Lisp expressions to JIT-compiled machine code.

use crate::ast::{Expr, BuiltinOp};
use crate::environment::LexicalAddress;
use crate::jit::VarBuilder;
use crate::symbol::Symbol;
use crate::var::Var;

use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{Linkage, Module};
use std::collections::HashMap;

/// Compilation context that tracks variables and their lexical addresses
#[derive(Debug, Clone)]
pub struct CompileContext {
    /// Variable bindings: symbol -> lexical address  
    bindings: HashMap<Symbol, LexicalAddress>,
    /// Current environment depth
    depth: u32,
}

impl CompileContext {
    /// Create a new empty compilation context
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            depth: 0,
        }
    }
    
    /// Look up a variable binding
    pub fn lookup(&self, var: Symbol) -> Option<LexicalAddress> {
        self.bindings.get(&var).copied()
    }
    
    /// Add a variable binding at the current depth
    pub fn bind(&mut self, var: Symbol, offset: u32) {
        let addr = LexicalAddress {
            depth: self.depth,
            offset,
        };
        self.bindings.insert(var, addr);
    }
    
    /// Create a new context with increased depth (for nested scopes)
    pub fn push_scope(&self) -> Self {
        Self {
            bindings: self.bindings.clone(),
            depth: self.depth + 1,
        }
    }
}

/// The main compiler for converting AST to executable functions
pub struct Compiler {
    module: JITModule,
    var_builder: VarBuilder,
    ctx: codegen::Context,
    builder_context: FunctionBuilderContext,
}

impl Compiler {
    /// Create a new compiler instance
    pub fn new() -> Self {
        // Use the same ISA detection as our existing JIT infrastructure
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {msg}");
        });
        let isa = isa_builder.finish(settings::Flags::new(settings::builder())).unwrap();
        
        let builder = cranelift_jit::JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let module = JITModule::new(builder);
        let var_builder = VarBuilder::new();
        
        Self {
            module,
            var_builder,
            ctx: codegen::Context::new(),
            builder_context: FunctionBuilderContext::new(),
        }
    }
    
    /// Compile an expression to a function that returns a Var (as u64)
    /// The function signature is: fn(env: u64) -> u64
    pub fn compile_expr(&mut self, expr: &Expr) -> Result<*const u8, String> {
        // Create function signature: (env: u64) -> u64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // environment parameter
        sig.returns.push(AbiParam::new(types::I64)); // return Var as u64
        
        // Generate unique function name to avoid conflicts
        use std::sync::atomic::{AtomicU32, Ordering};
        static FUNC_COUNTER: AtomicU32 = AtomicU32::new(0);
        let func_name = format!("compiled_expr_{}", FUNC_COUNTER.fetch_add(1, Ordering::SeqCst));
        
        // Create the function
        let func_id = self.module
            .declare_function(&func_name, Linkage::Export, &sig)
            .map_err(|e| format!("Failed to declare function: {e}"))?;
            
        // Clear the context and set up function
        self.ctx.clear();
        self.ctx.func.signature = sig;
        
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_context);
        
        // Create entry block
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        
        // Get the environment parameter
        let env_param = builder.block_params(entry_block)[0];
        
        // Compile the expression
        let ctx = CompileContext::new();
        let var_builder = &self.var_builder;
        let result = compile_expr_recursive(expr, &mut builder, env_param, &ctx, var_builder)?;
        
        // Return the result
        builder.ins().return_(&[result]);
        
        // Finalize the function
        builder.finalize();
        
        // Define the function in the module
        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define function: {e}"))?;
            
        // Finalize the module and get the function pointer
        self.module
            .finalize_definitions()
            .map_err(|e| format!("Failed to finalize: {e}"))?;
            
        let code_ptr = self.module.get_finalized_function(func_id);
        Ok(code_ptr)
    }
    
}

/// Standalone recursive expression compiler to avoid borrowing conflicts
fn compile_expr_recursive(
    expr: &Expr,
    builder: &mut FunctionBuilder,
    env: Value,
    ctx: &CompileContext,
    var_builder: &VarBuilder,
) -> Result<Value, String> {
    match expr {
        Expr::Literal(var) => {
            // Load literal value as u64
            let bits = var.as_u64();
            Ok(builder.ins().iconst(types::I64, bits as i64))
        }
        
        Expr::Variable(sym) => {
            // Look up variable in environment
            if let Some(addr) = ctx.lookup(*sym) {
                // Call env_get(env, depth, offset) -> u64
                let _depth_val = builder.ins().iconst(types::I32, addr.depth as i64);
                let _offset_val = builder.ins().iconst(types::I32, addr.offset as i64);
                // TODO: Call the actual env_get function
                // For now, return a placeholder
                Ok(builder.ins().iconst(types::I64, 0))
            } else {
                Err(format!("Unbound variable: {}", sym.as_string()))
            }
        }
        
        Expr::Call { func, args } => {
            // Check if it's a builtin operation
            if let Expr::Variable(sym) = func.as_ref() {
                if let Some(builtin) = BuiltinOp::from_symbol(*sym) {
                    return compile_builtin_recursive(builtin, args, builder, env, ctx, var_builder);
                }
            }
            
            // TODO: User-defined function calls
            Err("User-defined function calls not yet implemented".to_string())
        }
        
        Expr::Let { bindings, body } => {
            // Create new environment with space for bindings
            let _slot_count = bindings.len() as u32;
            let _count_val = builder.ins().iconst(types::I32, _slot_count as i64);
            // TODO: Call the actual env_create function
            // For now, use the parent environment
            let new_env = env;
            
            // Create new context for the let body
            let mut new_ctx = ctx.push_scope();
            
            // Compile and store each binding
            for (i, (var, expr)) in bindings.iter().enumerate() {
                // Compile the binding expression in the outer context
                let _value = compile_expr_recursive(expr, builder, env, ctx, var_builder)?;
                
                // TODO: Store in the new environment
                let _depth_val = builder.ins().iconst(types::I32, 0);
                let _offset_val = builder.ins().iconst(types::I32, i as i64);
                
                // Add to new context
                new_ctx.bind(*var, i as u32);
            }
            
            // Compile the body with the new environment and context
            compile_expr_recursive(body, builder, new_env, &new_ctx, var_builder)
        }
        
        Expr::Lambda { .. } => {
            // TODO: Lambda compilation requires closure creation
            Err("Lambda expressions not yet implemented".to_string())
        }
        
        Expr::If { condition, then_expr, else_expr } => {
            // Compile condition
            let cond_value = compile_expr_recursive(condition, builder, env, ctx, var_builder)?;
            
            // Check if condition is truthy by comparing against encoded 0.0
            // TODO: Implement proper Var truthiness check using VarBuilder
            let encoded_false = builder.ins().iconst(types::I64, Var::float(0.0).as_u64() as i64);
            let is_true = builder.ins().icmp(IntCC::NotEqual, cond_value, encoded_false);
            
            // Create blocks
            let then_block = builder.create_block();
            let else_block = builder.create_block();
            let merge_block = builder.create_block();
            
            // Add block parameter for the result
            builder.append_block_param(merge_block, types::I64);
            
            // Branch based on condition
            builder.ins().brif(is_true, then_block, &[], else_block, &[]);
            
            // Compile then branch
            builder.switch_to_block(then_block);
            builder.seal_block(then_block);
            let then_result = compile_expr_recursive(then_expr, builder, env, ctx, var_builder)?;
            builder.ins().jump(merge_block, &[then_result]);
            
            // Compile else branch
            builder.switch_to_block(else_block);
            builder.seal_block(else_block);
            let else_result = compile_expr_recursive(else_expr, builder, env, ctx, var_builder)?;
            builder.ins().jump(merge_block, &[else_result]);
            
            // Merge point
            builder.switch_to_block(merge_block);
            builder.seal_block(merge_block);
            
            Ok(builder.block_params(merge_block)[0])
        }
    }
}

/// Standalone builtin operation compiler
fn compile_builtin_recursive(
    op: BuiltinOp,
    args: &[Expr],
    builder: &mut FunctionBuilder,
    env: Value,
    ctx: &CompileContext,
    var_builder: &VarBuilder,
) -> Result<Value, String> {
    // Validate arity
    if let Some(expected_arity) = op.arity() {
        if args.len() != expected_arity {
            return Err(format!(
                "Wrong number of arguments for {:?}: expected {}, got {}",
                op, expected_arity, args.len()
            ));
        }
    }
    
    match op {
        BuiltinOp::Add => {
            let lhs = compile_expr_recursive(&args[0], builder, env, ctx, var_builder)?;
            let rhs = compile_expr_recursive(&args[1], builder, env, ctx, var_builder)?;
            let result = var_builder.emit_double_add(builder, lhs, rhs);
            Ok(result)
        }
        
        BuiltinOp::Sub => {
            // TODO: Implement subtraction
            Err("Subtraction not yet implemented".to_string())
        }
        
        BuiltinOp::Mul => {
            // TODO: Implement multiplication  
            Err("Multiplication not yet implemented".to_string())
        }
        
        BuiltinOp::Div => {
            // TODO: Implement division
            Err("Division not yet implemented".to_string())
        }
        
        BuiltinOp::Mod => {
            // TODO: Implement modulo
            Err("Modulo not yet implemented".to_string())
        }
        
        _ => {
            Err(format!("Builtin operation {op:?} not yet implemented"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;
    use crate::var::Var;
    
    #[test]
    fn test_compile_context() {
        let mut ctx = CompileContext::new();
        
        // Test variable binding
        let var = Symbol::mk("x");
        ctx.bind(var, 0);
        
        let addr = ctx.lookup(var).unwrap();
        assert_eq!(addr.depth, 0);
        assert_eq!(addr.offset, 0);
        
        // Test scope pushing
        let nested_ctx = ctx.push_scope();
        assert_eq!(nested_ctx.depth, 1);
    }
    
    #[test]
    fn test_compiler_creation() {
        let _compiler = Compiler::new();
        // Just test that we can create a compiler without panicking
    }
    
    #[test]
    fn test_literal_compilation() {
        let mut compiler = Compiler::new();
        let expr = Expr::number(42.0);
        
        // This should compile without error (though may not execute properly yet)
        let result = compiler.compile_expr(&expr);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_addition_compilation() {
        let mut compiler = Compiler::new();
        
        // Create (+ 1.0 2.0)
        let expr = Expr::call(
            Expr::variable("+"),
            vec![Expr::number(1.0), Expr::number(2.0)]
        );
        
        // Should compile successfully
        let result = compiler.compile_expr(&expr);
        assert!(result.is_ok(), "Addition compilation failed: {:?}", result.err());
        
        // Get the function pointer
        let func_ptr = result.unwrap();
        assert!(!func_ptr.is_null(), "Function pointer should not be null");
    }
    
    #[test]  
    fn test_nested_addition_compilation() {
        let mut compiler = Compiler::new();
        
        // Create (+ (+ 1.0 2.0) 3.0)
        let inner_add = Expr::call(
            Expr::variable("+"),
            vec![Expr::number(1.0), Expr::number(2.0)]
        );
        let outer_add = Expr::call(
            Expr::variable("+"),
            vec![inner_add, Expr::number(3.0)]
        );
        
        // Should compile successfully
        let result = compiler.compile_expr(&outer_add);
        assert!(result.is_ok(), "Nested addition compilation failed: {:?}", result.err());
    }
    
    #[test]
    fn test_variable_compilation_error() {
        let mut compiler = Compiler::new();
        
        // Create unbound variable reference
        let expr = Expr::variable("x");
        
        // Should fail compilation with unbound variable error
        let result = compiler.compile_expr(&expr);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("Unbound variable"));
    }
    
    #[test]
    fn test_let_binding_compilation() {
        let mut compiler = Compiler::new();
        
        // Create (let ((x 5.0)) (+ x 2.0))
        use crate::symbol::Symbol;
        let x_sym = Symbol::mk("x");
        let expr = Expr::let_binding(
            vec![(x_sym, Expr::number(5.0))],
            Expr::call(
                Expr::variable("+"),
                vec![Expr::variable("x"), Expr::number(2.0)]
            )
        );
        
        // Should compile successfully
        let result = compiler.compile_expr(&expr);
        assert!(result.is_ok(), "Let binding compilation failed: {:?}", result.err());
    }
    
    #[test]
    fn test_if_expression_compilation() {
        let mut compiler = Compiler::new();
        
        // Create (if 1 42.0 24.0) - non-zero condition should be truthy
        let expr = Expr::if_expr(
            Expr::number(1.0),
            Expr::number(42.0),
            Expr::number(24.0)
        );
        
        // Should compile successfully
        let result = compiler.compile_expr(&expr);
        assert!(result.is_ok(), "If expression compilation failed: {:?}", result.err());
    }
    
    #[test]
    fn test_invalid_builtin_arity() {
        let mut compiler = Compiler::new();
        
        // Create (+ 1.0) - addition requires 2 arguments
        let expr = Expr::call(
            Expr::variable("+"),
            vec![Expr::number(1.0)]
        );
        
        // Should fail compilation with arity error
        let result = compiler.compile_expr(&expr);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("Wrong number of arguments"));
    }
    
    #[test]
    fn test_unimplemented_builtin() {
        let mut compiler = Compiler::new();
        
        // Create (- 5.0 3.0) - subtraction not yet implemented
        let expr = Expr::call(
            Expr::variable("-"),
            vec![Expr::number(5.0), Expr::number(3.0)]
        );
        
        // Should fail compilation with unimplemented error
        let result = compiler.compile_expr(&expr);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("not yet implemented"));
    }
    
    #[test]
    fn test_execute_literal() {
        let mut compiler = Compiler::new();
        let expr = Expr::number(42.0);
        
        // Compile the expression
        let func_ptr = compiler.compile_expr(&expr).unwrap();
        
        // Cast to function and execute with null environment
        let func: fn(u64) -> u64 = unsafe { std::mem::transmute(func_ptr) };
        let result_bits = func(0); // null environment for literal
        
        // Convert result back to Var and check value
        let result_var = Var::from_u64(result_bits);
        assert_eq!(result_var.as_double(), Some(42.0));
    }
    
    #[test] 
    fn test_execute_addition() {
        let mut compiler = Compiler::new();
        
        // Create (+ 1.0 2.0)
        let expr = Expr::call(
            Expr::variable("+"),
            vec![Expr::number(1.0), Expr::number(2.0)]
        );
        
        // Compile and execute
        let func_ptr = compiler.compile_expr(&expr).unwrap();
        let func: fn(u64) -> u64 = unsafe { std::mem::transmute(func_ptr) };
        let result_bits = func(0); // null environment
        
        // Check result
        let result_var = Var::from_u64(result_bits);
        assert_eq!(result_var.as_double(), Some(3.0));
    }
    
    #[test]
    fn test_execute_nested_addition() {
        let mut compiler = Compiler::new();
        
        // Create (+ (+ 1.0 2.0) 3.0) = 6.0
        let inner_add = Expr::call(
            Expr::variable("+"),
            vec![Expr::number(1.0), Expr::number(2.0)]
        );
        let outer_add = Expr::call(
            Expr::variable("+"),
            vec![inner_add, Expr::number(3.0)]
        );
        
        // Compile and execute
        let func_ptr = compiler.compile_expr(&outer_add).unwrap();
        let func: fn(u64) -> u64 = unsafe { std::mem::transmute(func_ptr) };
        let result_bits = func(0);
        
        // Check result
        let result_var = Var::from_u64(result_bits);
        assert_eq!(result_var.as_double(), Some(6.0));
    }
    
    #[test]
    fn test_execute_if_expression() {
        let mut compiler = Compiler::new();
        
        // Test truthy condition: (if 1.0 42.0 24.0) should return 42.0
        let expr = Expr::if_expr(
            Expr::number(1.0),
            Expr::number(42.0), 
            Expr::number(24.0)
        );
        
        let func_ptr = compiler.compile_expr(&expr).unwrap();
        let func: fn(u64) -> u64 = unsafe { std::mem::transmute(func_ptr) };
        let result_bits = func(0);
        let result_var = Var::from_u64(result_bits);
        assert_eq!(result_var.as_double(), Some(42.0));
        
        // Test falsy condition: (if 0.0 42.0 24.0) should return 24.0  
        let expr_false = Expr::if_expr(
            Expr::number(0.0),
            Expr::number(42.0),
            Expr::number(24.0)
        );
        
        let func_ptr_false = compiler.compile_expr(&expr_false).unwrap();
        let func_false: fn(u64) -> u64 = unsafe { std::mem::transmute(func_ptr_false) };
        let result_bits_false = func_false(0);
        let result_var_false = Var::from_u64(result_bits_false);
        assert_eq!(result_var_false.as_double(), Some(24.0));
    }
}