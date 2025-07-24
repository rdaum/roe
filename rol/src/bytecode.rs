//! Stack-based bytecode virtual machine.
//! Optimized JIT compiler that analyzes bytecode sequences and generates efficient machine code.

use crate::ast::{Expr, BuiltinOp};
use crate::jit::VarBuilder;
use crate::symbol::Symbol;
use crate::var::Var;

use cranelift::prelude::*;
use cranelift::codegen::ir::StackSlot;
use cranelift_jit::JITModule;
use cranelift_module::{Linkage, Module};
use cranelift::codegen::ir::FuncRef;
use cranelift::codegen::isa::CallConv;

/// Runtime helper function for DefGlobal opcode
#[unsafe(no_mangle)]
pub extern "C" fn jit_set_global(jit_ptr: *mut BytecodeJIT, symbol_id: u64, value: u64) {
    unsafe {
        let jit = &mut *jit_ptr;
        let symbol = Symbol::from_id(symbol_id as u32);
        let var = Var::from_u64(value);
        jit.set_global(symbol, var);
    }
}

/// Runtime helper function for LoadVar opcode (global lookup)  
#[unsafe(no_mangle)]
pub extern "C" fn jit_get_global(jit_ptr: *mut BytecodeJIT, symbol_id: u64) -> u64 {
    unsafe {
        let jit = &*jit_ptr;
        let symbol = Symbol::from_id(symbol_id as u32);
        if let Some(var) = jit.get_global(symbol) {
            var.as_u64()
        } else {
            Var::none().as_u64() // Return none for undefined globals
        }
    }
}

/// Runtime helper function for creating closures from lambda expressions
#[unsafe(no_mangle)]
pub extern "C" fn jit_create_closure(_jit_ptr: *mut BytecodeJIT, arity: u32, _body_bytes: *const u8, _body_len: u64) -> u64 {
    // For now, create a simple closure that just stores arity
    // TODO: Implement proper lambda body compilation
    let closure_ptr = crate::heap::LispClosure::new(
        std::ptr::null(), // Function pointer - will be set later
        arity,
        0 // No captured environment for now
    );
    Var::closure(closure_ptr).as_u64()
}

/// Runtime helper function for calling closures
/// This is called from JIT-compiled code when a closure is invoked
#[unsafe(no_mangle)]
pub extern "C" fn jit_call_closure(jit_ptr: *mut BytecodeJIT, closure: u64, args_ptr: *const u64, arg_count: u32) -> u64 {
    unsafe {
        let jit = &mut *jit_ptr;
        let closure_var = Var::from_u64(closure);
        
        if let Some(closure_ptr) = closure_var.as_closure() {
            let closure_obj = &*closure_ptr;
            
            // Check if the function is already compiled
            if closure_obj.func_ptr.is_null() {
                // Need to compile the lambda on-demand
                let function_id = closure_obj.captured_env as u32; // We stored function_id here
                
                // Clone the registry to avoid borrowing conflicts
                let registry_clone = jit.lambda_registry.clone();
                
                if let Some(registry) = registry_clone {
                    if let Some((params, body)) = registry.get(&function_id) {
                        // Compile the lambda body now
                        match jit.compile_lambda_on_demand(params, body) {
                            Ok(compiled_ptr) => {
                                // Cache the compiled function pointer
                                jit.compiled_lambdas.insert(function_id, compiled_ptr);
                                
                                // Call the compiled function directly
                                let func: extern "C" fn(*const u64, u32, u64) -> u64 = std::mem::transmute(compiled_ptr);
                                return func(args_ptr, arg_count, 0);
                            }
                            Err(_) => {
                                return Var::none().as_u64();
                            }
                        }
                    }
                }
                
                // Lambda not found or compilation failed
                Var::none().as_u64()
            } else {
                // Function is already compiled, call it directly
                let func: extern "C" fn(*const u64, u32, u64) -> u64 = std::mem::transmute(closure_obj.func_ptr);
                func(args_ptr, arg_count, closure_obj.captured_env)
            }
        } else {
            // Not a valid closure
            Var::none().as_u64()
        }
    }
}

/// Bytecode instruction set for our stack-based VM
#[derive(Debug, Clone, PartialEq)]
pub enum Opcode {
    // === Stack Operations ===
    /// Push a constant value onto the stack
    LoadConst(Var),
    
    /// Push nil/none onto the stack
    LoadNil,
    
    // === Variables ===
    /// Push variable value onto the stack
    LoadVar(Symbol),
    
    /// Pop stack, store value in variable
    StoreVar(Symbol),
    
    /// Pop stack, store value in global variable (def)
    DefGlobal(Symbol),
    
    /// Push captured variable (upvalue) onto the stack
    LoadUpvalue(u8),
    
    // === Arithmetic Operations (pop 2, push 1) ===
    /// Pop two values, push their sum
    Add,
    
    /// Pop two values, push their difference (second - first)
    Sub,
    
    /// Pop two values, push their product
    Mul,
    
    /// Pop two values, push their quotient (second / first)
    Div,
    
    /// Pop two values, push their remainder (second % first)
    Mod,
    
    /// Pop two values, push boolean (second < first)
    Less,
    
    /// Pop two values, push boolean (second <= first)
    LessEqual,
    
    /// Pop two values, push boolean (second > first)
    Greater,
    
    /// Pop two values, push boolean (second >= first)
    GreaterEqual,
    
    /// Pop two values, push boolean (second == first)
    Equal,
    
    /// Pop two values, push boolean (second != first)
    NotEqual,
    
    /// Pop two values, push boolean (first && second)
    And,
    
    /// Pop two values, push boolean (first || second)
    Or,
    
    /// Pop one value, push boolean (!value)
    Not,
    
    /// Conditional select: pop else_val, then_val, condition; push then_val if condition is truthy, else else_val
    Select,
    
    // === Control Flow ===
    /// Unconditional jump to label
    Jump(Label),
    
    /// Pop stack, jump to label if value is truthy
    JumpIf(Label),
    
    /// Pop stack, jump to label if value is falsy
    JumpIfNot(Label),
    
    // === Function Calls ===
    /// Call function: pop function and N arguments, push result
    Call(u8),
    
    /// Tail call optimization: pop function and N arguments, return result
    TailCall(u8),
    
    /// Return: pop stack value and return it
    Return,
    
    // === Closures ===
    /// Create closure: pop N upvalues from stack, push closure
    Closure(FunctionId, u8),
    
    // === Environment Management ===
    /// Create new lexical scope with N variable slots
    PushScope(u8),
    
    /// Destroy current lexical scope
    PopScope,
    
    /// Jump target label - marks a position in the bytecode
    Label(Label),
}

/// Jump target label (offset into bytecode)
pub type Label = u32;

/// Reference to a compiled function
pub type FunctionId = u32;

/// A compiled function containing bytecode
#[derive(Debug, Clone)]
pub struct Function {
    /// Unique identifier for this function
    pub id: FunctionId,
    
    /// Function name (for debugging)
    pub name: Option<Symbol>,
    
    /// Number of parameters this function expects
    pub arity: u8,
    
    /// Number of upvalues (captured variables) this function uses
    pub upvalue_count: u8,
    
    /// The bytecode instructions
    pub code: Vec<Opcode>,
    
    /// Constants referenced by the function
    pub constants: Vec<Var>,
}

impl Function {
    /// Create a new function
    pub fn new(id: FunctionId, name: Option<Symbol>, arity: u8, upvalue_count: u8) -> Self {
        Self {
            id,
            name,
            arity,
            upvalue_count,
            code: Vec::new(),
            constants: Vec::new(),
        }
    }
    
    /// Add an instruction to this function
    pub fn emit(&mut self, opcode: Opcode) {
        self.code.push(opcode);
    }
    
    /// Add a constant and return its index
    pub fn add_constant(&mut self, value: Var) -> usize {
        self.constants.push(value);
        self.constants.len() - 1
    }
}

/// Compiler from AST to bytecode
pub struct BytecodeCompiler {
    /// Function being compiled
    function: Function,
    
    /// Next function ID to assign
    next_function_id: FunctionId,
    
    /// Label counter for generating unique jump labels
    next_label: Label,
    
    /// Lambda functions awaiting compilation (function_id -> (params, body))
    pub lambda_registry: std::collections::HashMap<FunctionId, (Vec<Symbol>, Expr)>,
}

impl BytecodeCompiler {
    /// Create a new bytecode compiler
    pub fn new() -> Self {
        Self {
            function: Function::new(0, None, 0, 0),
            next_function_id: 1,
            next_label: 0,
            lambda_registry: std::collections::HashMap::new(),
        }
    }
    
    /// Compile an expression to bytecode
    pub fn compile_expr(&mut self, expr: &Expr) -> Result<Function, String> {
        // Reset for new compilation
        self.function = Function::new(0, None, 0, 0);
        
        // Compile the expression
        self.compile_expr_recursive(expr)?;
        
        // Add return instruction
        self.function.emit(Opcode::Return);
        
        Ok(self.function.clone())
    }
    
    /// Recursively compile an expression
    fn compile_expr_recursive(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Literal(var) => {
                // Push literal value onto stack
                self.function.emit(Opcode::LoadConst(*var));
                Ok(())
            }
            
            Expr::Variable(symbol) => {
                // Load variable onto stack
                self.function.emit(Opcode::LoadVar(*symbol));
                Ok(())
            }
            
            Expr::Call { func, args } => {
                // Check if this is a builtin operation
                if let Expr::Variable(sym) = func.as_ref() {
                    if let Some(builtin) = BuiltinOp::from_symbol(*sym) {
                        return self.compile_builtin_op(builtin, args);
                    }
                }
                
                // Regular function call
                // Compile function expression (should push function onto stack)
                self.compile_expr_recursive(func)?;
                
                // Compile all arguments (pushes them onto stack in order)
                for arg in args {
                    self.compile_expr_recursive(arg)?;
                }
                
                // Call with argument count
                self.function.emit(Opcode::Call(args.len() as u8));
                Ok(())
            }
            
            Expr::Let { bindings, body } => {
                // Create new scope
                self.function.emit(Opcode::PushScope(bindings.len() as u8));
                
                // Compile and store each binding
                for (symbol, value_expr) in bindings {
                    self.compile_expr_recursive(value_expr)?;
                    self.function.emit(Opcode::StoreVar(*symbol));
                }
                
                // Compile body
                self.compile_expr_recursive(body)?;
                
                // Clean up scope
                self.function.emit(Opcode::PopScope);
                Ok(())
            }
            
            Expr::If { condition, then_expr, else_expr } => {
                // Emit proper conditional branching using jumps
                // Only the taken branch should ever be executed
                
                // Generate unique labels for this if statement
                let else_label = self.next_label;
                self.next_label += 1;
                let end_label = self.next_label;
                self.next_label += 1;
                
                // Compile condition
                self.compile_expr_recursive(condition)?;
                
                // Jump to else branch if condition is falsy
                self.function.emit(Opcode::JumpIfNot(else_label));
                
                // Compile then branch
                self.compile_expr_recursive(then_expr)?;
                
                // Jump to end to skip else branch
                self.function.emit(Opcode::Jump(end_label));
                
                // Else branch starts here (label: else_label)
                self.function.emit(Opcode::Label(else_label));
                self.compile_expr_recursive(else_expr)?;
                
                // End of if statement (label: end_label)
                self.function.emit(Opcode::Label(end_label));
                
                Ok(())
            }
            
            Expr::While { condition, body } => {
                // Generate unique labels for this while loop
                let loop_start_label = self.next_label;
                self.next_label += 1;
                let loop_end_label = self.next_label;
                self.next_label += 1;
                
                // Loop start (label: loop_start_label)
                self.function.emit(Opcode::Label(loop_start_label));
                
                // Compile condition
                self.compile_expr_recursive(condition)?;
                
                // Exit loop if condition is falsy
                self.function.emit(Opcode::JumpIfNot(loop_end_label));
                
                // Compile body
                self.compile_expr_recursive(body)?;
                
                // Jump back to start of loop
                self.function.emit(Opcode::Jump(loop_start_label));
                
                // End of loop (label: loop_end_label)
                self.function.emit(Opcode::Label(loop_end_label));
                
                // While loops return nil
                self.function.emit(Opcode::LoadConst(Var::none()));
                
                Ok(())
            }
            
            Expr::For { var, start, end, body } => {
                // Generate unique labels for this for loop
                let loop_start_label = self.next_label;
                self.next_label += 1;
                let loop_end_label = self.next_label;
                self.next_label += 1;
                
                // Create new scope for loop variable
                self.function.emit(Opcode::PushScope(1));
                
                // Initialize loop variable with start value
                self.compile_expr_recursive(start)?;
                self.function.emit(Opcode::StoreVar(*var));
                
                // Loop start (label: loop_start_label)
                self.function.emit(Opcode::Label(loop_start_label));
                
                // Check if loop variable < end
                self.function.emit(Opcode::LoadVar(*var));
                self.compile_expr_recursive(end)?;
                self.function.emit(Opcode::Less);
                
                // Exit loop if condition is falsy (var >= end)
                self.function.emit(Opcode::JumpIfNot(loop_end_label));
                
                // Compile body
                self.compile_expr_recursive(body)?;
                
                // Increment loop variable: var = var + 1
                self.function.emit(Opcode::LoadVar(*var));
                self.function.emit(Opcode::LoadConst(Var::int(1)));
                self.function.emit(Opcode::Add);
                self.function.emit(Opcode::StoreVar(*var));
                
                // Jump back to start of loop
                self.function.emit(Opcode::Jump(loop_start_label));
                
                // End of loop (label: loop_end_label)
                self.function.emit(Opcode::Label(loop_end_label));
                
                // Clean up scope
                self.function.emit(Opcode::PopScope);
                
                // For loops return nil
                self.function.emit(Opcode::LoadConst(Var::none()));
                
                Ok(())
            }
            
            Expr::Lambda { params, body } => {
                // Register this lambda for later compilation
                let function_id = self.next_function_id;
                self.next_function_id += 1;
                
                // Store the lambda information for JIT compilation
                self.lambda_registry.insert(function_id, (params.clone(), *body.clone()));
                
                // Emit a Closure opcode that will trigger lambda compilation at JIT time
                // For now, assume no upvalues (captured variables)
                let upvalue_count = 0;
                self.function.emit(Opcode::Closure(function_id, upvalue_count));
                Ok(())
            }
            
            Expr::Def { var, value } => {
                // Compile the value expression
                self.compile_expr_recursive(value)?;
                
                // Define global variable
                self.function.emit(Opcode::DefGlobal(*var));
                
                // Return the defined value
                self.function.emit(Opcode::LoadVar(*var));
                Ok(())
            }
            
            Expr::VarDef { var, value } => {
                // Compile the value expression
                self.compile_expr_recursive(value)?;
                
                // Define mutable global variable (for now, same as DefGlobal)
                self.function.emit(Opcode::DefGlobal(*var));
                
                // Return the defined value
                self.function.emit(Opcode::LoadVar(*var));
                Ok(())
            }
            
            Expr::Defn { name, params, body } => {
                // Compile defn as syntactic sugar: (def name (lambda [params...] body))
                // Create a lambda expression
                let lambda_expr = Expr::Lambda {
                    params: params.clone(),
                    body: body.clone(),
                };
                
                // Compile the lambda expression
                self.compile_expr_recursive(&lambda_expr)?;
                
                // Define the function globally
                self.function.emit(Opcode::DefGlobal(*name));
                
                // Return the function
                self.function.emit(Opcode::LoadVar(*name));
                Ok(())
            }
        }
    }
    
    /// Compile a builtin operation
    fn compile_builtin_op(&mut self, op: BuiltinOp, args: &[Expr]) -> Result<(), String> {
        // Validate argument count
        if let Some(expected_arity) = op.arity() {
            if args.len() != expected_arity {
                return Err(format!("Builtin {} expects {} arguments, got {}", 
                    self.builtin_name(op), expected_arity, args.len()));
            }
        }
        
        // Compile arguments (they get pushed onto stack)
        for arg in args {
            self.compile_expr_recursive(arg)?;
        }
        
        // Emit the corresponding opcode
        match op {
            BuiltinOp::Add => self.function.emit(Opcode::Add),
            BuiltinOp::Sub => self.function.emit(Opcode::Sub),
            BuiltinOp::Mul => self.function.emit(Opcode::Mul),
            BuiltinOp::Div => self.function.emit(Opcode::Div),
            BuiltinOp::Mod => self.function.emit(Opcode::Mod),
            BuiltinOp::Lt => self.function.emit(Opcode::Less),
            BuiltinOp::Le => self.function.emit(Opcode::LessEqual),
            BuiltinOp::Gt => self.function.emit(Opcode::Greater),
            BuiltinOp::Ge => self.function.emit(Opcode::GreaterEqual),
            BuiltinOp::Eq => self.function.emit(Opcode::Equal),
            BuiltinOp::Ne => self.function.emit(Opcode::NotEqual),
            BuiltinOp::And => self.function.emit(Opcode::And),
            BuiltinOp::Or => self.function.emit(Opcode::Or),
            BuiltinOp::Not => self.function.emit(Opcode::Not),
        }
        
        Ok(())
    }
    
    /// Get builtin operation name for error messages
    fn builtin_name(&self, op: BuiltinOp) -> &'static str {
        match op {
            BuiltinOp::Add => "+",
            BuiltinOp::Sub => "-",
            BuiltinOp::Mul => "*",
            BuiltinOp::Div => "/",
            BuiltinOp::Mod => "%",
            BuiltinOp::Eq => "=",
            BuiltinOp::Ne => "!=",
            BuiltinOp::Lt => "<",
            BuiltinOp::Le => "<=",
            BuiltinOp::Gt => ">",
            BuiltinOp::Ge => ">=",
            BuiltinOp::And => "and",
            BuiltinOp::Or => "or",
            BuiltinOp::Not => "not",
        }
    }
}

/// Optimizing JIT compiler that converts bytecode to machine code
pub struct BytecodeJIT {
    module: JITModule,
    var_builder: VarBuilder,
    ctx: codegen::Context,
    builder_context: FunctionBuilderContext,
    function_counter: u32,
    /// Global variables that persist between REPL evaluations
    global_variables: std::collections::HashMap<Symbol, Var>,
    /// Lambda registry available during execution
    lambda_registry: Option<std::collections::HashMap<FunctionId, (Vec<Symbol>, Expr)>>,
    /// Cache of compiled lambda function pointers to avoid recompilation
    compiled_lambdas: std::collections::HashMap<FunctionId, *const u8>,
}

impl BytecodeJIT {
    /// Create a new bytecode JIT compiler
    pub fn new() -> Self {
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {msg}");
        });
        let isa = isa_builder.finish(settings::Flags::new(settings::builder())).unwrap();
        
        let mut builder = cranelift_jit::JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        
        // Register our runtime helper functions with the JIT
        builder.symbol("jit_set_global", jit_set_global as *const u8);
        builder.symbol("jit_get_global", jit_get_global as *const u8);
        builder.symbol("jit_create_closure", jit_create_closure as *const u8);
        builder.symbol("jit_call_closure", jit_call_closure as *const u8);
        
        let module = JITModule::new(builder);
        
        Self {
            module,
            var_builder: VarBuilder::new(),
            ctx: codegen::Context::new(),
            builder_context: FunctionBuilderContext::new(),
            function_counter: 0,
            global_variables: std::collections::HashMap::new(),
            lambda_registry: None,
            compiled_lambdas: std::collections::HashMap::new(),
        }
    }
    
    /// Set a global variable value
    pub fn set_global(&mut self, symbol: Symbol, value: Var) {
        self.global_variables.insert(symbol, value);
    }
    
    /// Get a global variable value
    pub fn get_global(&self, symbol: Symbol) -> Option<Var> {
        self.global_variables.get(&symbol).cloned()
    }
    
    /// Get all global variables
    pub fn get_globals(&self) -> &std::collections::HashMap<Symbol, Var> {
        &self.global_variables
    }
    
    /// Execute a compiled function with this JIT as context
    pub fn execute_function(&mut self, func_ptr: *const u8) -> Var {
        let func: fn(*mut BytecodeJIT) -> u64 = unsafe { std::mem::transmute(func_ptr) };
        let result_bits = func(self as *mut BytecodeJIT);
        Var::from_u64(result_bits)
    }
    
    /// Compile a lambda function body to machine code
    pub fn compile_lambda(&mut self, params: &[Symbol], body: &Expr) -> Result<*const u8, String> {
        // Create function signature: fn(args: *const Var, arg_count: u32, captured_env: u64) -> u64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // args pointer
        sig.params.push(AbiParam::new(types::I32)); // arg count
        sig.params.push(AbiParam::new(types::I64)); // captured environment
        sig.returns.push(AbiParam::new(types::I64)); // return Var as u64
        
        // Generate unique function name
        let func_name = format!("lambda_{}", self.function_counter);
        self.function_counter += 1;
        
        // Create the function
        let func_id = self.module
            .declare_function(&func_name, Linkage::Export, &sig)
            .map_err(|e| format!("Failed to declare lambda function: {e}"))?;
            
        // Clear the context and set up function
        self.ctx.clear();
        self.ctx.func.signature = sig;
        
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_context);
        
        // Create entry block
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        
        // Get function parameters
        let args_ptr = builder.block_params(entry_block)[0];
        let _arg_count = builder.block_params(entry_block)[1];
        let _captured_env_param = builder.block_params(entry_block)[2];
        
        // Create a parameter lookup mechanism
        // We need to load parameters from the args array based on their index
        let mut param_values = std::collections::HashMap::new();
        for (i, &param_symbol) in params.iter().enumerate() {
            // Load the parameter from the args array: args[i]
            let param_offset = builder.ins().iconst(types::I64, i as i64);
            let param_ptr = builder.ins().iadd(args_ptr, param_offset);
            let param_value = builder.ins().load(types::I64, MemFlags::new(), param_ptr, 0);
            param_values.insert(param_symbol, param_value);
        }
        
        // Compile the lambda body with parameter bindings using native stack approach
        let mut compiler = BytecodeCompiler::new();
        let function = compiler.compile_expr(body)?;
        
        // Create an analyzer to compile opcodes using native stack
        let mut analyzer = BytecodeAnalyzer {
            var_builder: &self.var_builder,
            variables: std::collections::HashMap::new(),
            scope_stack: Vec::new(),
            label_blocks: std::collections::HashMap::new(),
            jit_ptr: None,
            set_global_ref: None,
            get_global_ref: None,
            lambda_registry: None,
            stack_base: None,
            stack_ptr_slot: None,
            stack_size: 1024,
        };
        
        // Pre-populate analyzer variables with parameter mappings
        for (symbol, &value) in &param_values {
            analyzer.variables.insert(*symbol, value);
        }
        
        // Use the proper sequence compilation that handles jumps correctly and uses native stack
        let result = analyzer.compile_sequence(&mut builder, &function.code)?;
        
        // Return the result
        builder.ins().return_(&[result]);
        
        // Finalize the function
        builder.finalize();
        
        // Define the function in the module
        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define lambda function: {e}"))?;
            
        // Finalize the module and get the function pointer
        self.module
            .finalize_definitions()
            .map_err(|e| format!("Failed to finalize lambda: {e}"))?;
            
        let code_ptr = self.module.get_finalized_function(func_id);
        Ok(code_ptr)
    }
    
    /// Simplified lambda body compilation for basic expressions
    
    /// Compile a lambda expression on-demand during JIT compilation
    /// This method is designed to avoid borrowing conflicts during compilation
    fn compile_lambda_on_demand(&mut self, params: &[Symbol], body: &Expr) -> Result<*const u8, String> {
        // Create a separate compilation context to avoid borrowing conflicts
        let mut lambda_ctx = codegen::Context::new();
        let mut lambda_builder_context = FunctionBuilderContext::new();
        
        // Create function signature: fn(args: *const Var, arg_count: u32, captured_env: u64) -> u64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // args pointer
        sig.params.push(AbiParam::new(types::I32)); // arg count
        sig.params.push(AbiParam::new(types::I64)); // captured environment
        sig.returns.push(AbiParam::new(types::I64)); // return Var as u64
        
        // Generate unique function name
        let func_name = format!("lambda_{}", self.function_counter);
        self.function_counter += 1;
        
        let func_id = self.module
            .declare_function(&func_name, Linkage::Export, &sig)
            .map_err(|e| format!("Failed to declare lambda function: {e}"))?;
            
        lambda_ctx.clear();
        lambda_ctx.func.signature = sig;
        
        // Declare global access functions for this lambda context BEFORE creating builder
        let set_global_sig = {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // jit_ptr
            sig.params.push(AbiParam::new(types::I64)); // symbol_id
            sig.params.push(AbiParam::new(types::I64)); // value
            sig
        };
        
        let get_global_sig = {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // jit_ptr
            sig.params.push(AbiParam::new(types::I64)); // symbol_id
            sig.returns.push(AbiParam::new(types::I64)); // value
            sig
        };
        
        let set_global_func = self.module
            .declare_function("jit_set_global", Linkage::Import, &set_global_sig)
            .map_err(|e| format!("Failed to declare set_global in lambda: {e}"))?;
            
        let get_global_func = self.module
            .declare_function("jit_get_global", Linkage::Import, &get_global_sig)
            .map_err(|e| format!("Failed to declare get_global in lambda: {e}"))?;
        
        let set_global_ref = self.module.declare_func_in_func(set_global_func, &mut lambda_ctx.func);
        let get_global_ref = self.module.declare_func_in_func(get_global_func, &mut lambda_ctx.func);
        
        let mut builder = FunctionBuilder::new(&mut lambda_ctx.func, &mut lambda_builder_context);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        
        // Get function parameters
        let args_ptr = builder.block_params(entry_block)[0];  
        let _arg_count = builder.block_params(entry_block)[1];
        let _captured_env = builder.block_params(entry_block)[2];
        
        // Create JIT pointer value for global access
        let jit_ptr_val = builder.ins().iconst(types::I64, self as *const BytecodeJIT as i64);
        
        // Create parameter bindings by loading from args array
        let mut param_values = std::collections::HashMap::new();
        for (i, param) in params.iter().enumerate() {
            // Load argument from args array: args_ptr[i]
            let arg_offset = builder.ins().iconst(types::I64, (i * 8) as i64); // Each Var is 8 bytes
            let arg_addr = builder.ins().iadd(args_ptr, arg_offset);
            let arg_value = builder.ins().load(types::I64, MemFlags::trusted(), arg_addr, 0);
            param_values.insert(*param, arg_value);
        }
        
        // Use the existing proper bytecode compilation path 
        // Compile the AST to bytecode first
        let mut compiler = BytecodeCompiler::new();
        let function = compiler.compile_expr(body)?;
        
        // Create an analyzer to compile opcodes with global access
        let mut analyzer = BytecodeAnalyzer {
            var_builder: &self.var_builder,
            variables: std::collections::HashMap::new(),
            scope_stack: Vec::new(),
            label_blocks: std::collections::HashMap::new(),
            jit_ptr: Some(jit_ptr_val),
            set_global_ref: Some(set_global_ref),
            get_global_ref: Some(get_global_ref),
            lambda_registry: self.lambda_registry.as_ref(),
            stack_base: None,
            stack_ptr_slot: None,
            stack_size: 1024,
        };
        
        // Pre-populate analyzer with global variables as constants
        for (symbol, var) in &self.global_variables {
            let const_value = builder.ins().iconst(types::I64, var.as_u64() as i64);
            analyzer.variables.insert(*symbol, const_value);
        }
        
        // Pre-populate analyzer variables with parameter mappings
        for (symbol, &value) in &param_values {
            analyzer.variables.insert(*symbol, value);
        }
        
        // Use the proper sequence compilation that handles jumps correctly
        let result_value = analyzer.compile_sequence(&mut builder, &function.code)?;
        
        // Return the result
        builder.ins().return_(&[result_value]);
        
        // Finalize the function
        builder.finalize();
        
        // Define the function in the module
        self.module
            .define_function(func_id, &mut lambda_ctx)
            .map_err(|e| format!("Failed to define lambda function: {e}"))?;
            
        self.module.clear_context(&mut lambda_ctx);
        self.module.finalize_definitions()
            .map_err(|e| format!("Failed to finalize lambda definitions: {e}"))?;
            
        // Get the finalized function pointer
        let func_ptr = self.module.get_finalized_function(func_id);
        Ok(func_ptr)
    }
    
    /// Compile bytecode function to optimized machine code  
    pub fn compile_function(&mut self, function: &Function, lambda_registry: &std::collections::HashMap<FunctionId, (Vec<Symbol>, Expr)>) -> Result<*const u8, String> {
        // Store lambda registry for use during compilation
        self.lambda_registry = Some(lambda_registry.clone());
        
        // Create function signature: (jit_ptr: *mut BytecodeJIT) -> u64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // JIT context pointer
        sig.returns.push(AbiParam::new(types::I64)); // return Var as u64
        
        // Generate unique function name using our counter
        let func_name = format!("bytecode_func_{}", self.function_counter);
        self.function_counter += 1;
        
        let func_id = self.module
            .declare_function(&func_name, Linkage::Export, &sig)
            .map_err(|e| format!("Failed to declare function: {e}"))?;
            
        self.ctx.clear();
        self.ctx.func.signature = sig;
        
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_context);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        
        // Get the JIT context pointer parameter
        let jit_ptr = builder.block_params(entry_block)[0];
        
        // Declare external runtime helper functions
        let set_global_sig = {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // jit_ptr
            sig.params.push(AbiParam::new(types::I64)); // symbol_id
            sig.params.push(AbiParam::new(types::I64)); // value
            sig
        };
        
        let get_global_sig = {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // jit_ptr
            sig.params.push(AbiParam::new(types::I64)); // symbol_id
            sig.returns.push(AbiParam::new(types::I64)); // value
            sig
        };
        
        let set_global_func = self.module
            .declare_function("jit_set_global", Linkage::Import, &set_global_sig)
            .map_err(|e| format!("Failed to declare set_global: {e}"))?;
            
        let get_global_func = self.module
            .declare_function("jit_get_global", Linkage::Import, &get_global_sig)
            .map_err(|e| format!("Failed to declare get_global: {e}"))?;
        
        let set_global_ref = self.module.declare_func_in_func(set_global_func, builder.func);
        let get_global_ref = self.module.declare_func_in_func(get_global_func, builder.func);
        
        // Analyze and compile the bytecode optimally
        let _result = {
            let mut analyzer = BytecodeAnalyzer::with_globals(&self.var_builder, jit_ptr, set_global_ref, get_global_ref, self.lambda_registry.as_ref());
            
            // Pre-populate analyzer with global variables as constants
            for (symbol, var) in &self.global_variables {
                let const_value = builder.ins().iconst(types::I64, var.as_u64() as i64);
                analyzer.variables.insert(*symbol, const_value);
            }
            
            match analyzer.compile_sequence(&mut builder, &function.code) {
                Ok(result) => {
                    builder.ins().return_(&[result]);
                    builder.finalize();
                    result
                }
                Err(e) => {
                    // Always finalize the builder, even on error, to keep Cranelift happy
                    // Use a dummy return value
                    let dummy_result = self.var_builder.make_none(&mut builder);
                    builder.ins().return_(&[dummy_result]);
                    builder.finalize();
                    return Err(e);
                }
            }
        };
        
        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define function: {e}"))?;
            
        self.module
            .finalize_definitions()
            .map_err(|e| format!("Failed to finalize: {e}"))?;
            
        let code_ptr = self.module.get_finalized_function(func_id);
        
        // Note: Keep lambda_registry available for runtime lambda compilation
        
        Ok(code_ptr)
    }
}

/// Helper function for lambda body compilation to avoid borrowing issues

/// Analyzes bytecode sequences and compiles them to optimized machine code
struct BytecodeAnalyzer<'a> {
    var_builder: &'a VarBuilder,
    variables: std::collections::HashMap<Symbol, Value>,
    scope_stack: Vec<Vec<Symbol>>,
    label_blocks: std::collections::HashMap<Label, Block>,
    jit_ptr: Option<Value>,
    set_global_ref: Option<FuncRef>,
    get_global_ref: Option<FuncRef>,
    lambda_registry: Option<&'a std::collections::HashMap<FunctionId, (Vec<Symbol>, Expr)>>,
    
    // Native stack management
    stack_base: Option<Value>,      // Base pointer to stack memory region
    stack_ptr_slot: Option<StackSlot>, // Stack slot holding current stack pointer
    stack_size: usize,              // Size of stack in bytes (each Var is 16 bytes)
}

impl<'a> BytecodeAnalyzer<'a> {
    
    fn with_globals(var_builder: &'a VarBuilder, jit_ptr: Value, set_global_ref: FuncRef, get_global_ref: FuncRef, lambda_registry: Option<&'a std::collections::HashMap<FunctionId, (Vec<Symbol>, Expr)>>) -> Self {
        Self {
            var_builder,
            variables: std::collections::HashMap::new(),
            scope_stack: Vec::new(),
            label_blocks: std::collections::HashMap::new(),
            jit_ptr: Some(jit_ptr),
            set_global_ref: Some(set_global_ref),
            get_global_ref: Some(get_global_ref),
            lambda_registry,
            stack_base: None,
            stack_ptr_slot: None,
            stack_size: 1024, // Default 1024 Var slots (8KB)
        }
    }
    
    /// Initialize native stack management for the current function
    fn init_native_stack(&mut self, builder: &mut FunctionBuilder) -> Result<(), String> {
        use cranelift::prelude::*;
        
        const VAR_SIZE: i32 = 8; // Size of Var struct in bytes (64-bit)
        let stack_bytes = (self.stack_size * VAR_SIZE as usize) as u32;
        
        // Use Cranelift's stack allocation instead of malloc
        // Allocate a large stack slot for our VM stack
        let stack_slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, stack_bytes));
        
        // Get the address of the stack slot as our stack base
        let stack_base = builder.ins().stack_addr(types::I64, stack_slot, 0);
        self.stack_base = Some(stack_base);
        
        // Allocate stack slot to hold current stack pointer (starts at base)
        self.stack_ptr_slot = Some(builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8)));
        
        // Initialize stack pointer to base
        builder.ins().stack_store(stack_base, self.stack_ptr_slot.unwrap(), 0);
        
        Ok(())
    }
    
    /// Push a value onto the native stack
    fn native_push(&mut self, builder: &mut FunctionBuilder, value: Value) -> Result<(), String> {
        use cranelift::prelude::*;
        
        let stack_ptr_slot = self.stack_ptr_slot.ok_or("Native stack not initialized")?;
        const VAR_SIZE: i32 = 8;
        
        // Load current stack pointer
        let stack_ptr = builder.ins().stack_load(types::I64, stack_ptr_slot, 0);
        
        // Store value at current stack pointer
        builder.ins().store(MemFlags::new(), value, stack_ptr, 0);
        
        // Increment stack pointer
        let new_stack_ptr = builder.ins().iadd_imm(stack_ptr, VAR_SIZE as i64);
        builder.ins().stack_store(new_stack_ptr, stack_ptr_slot, 0);
        
        Ok(())
    }
    
    /// Pop a value from the native stack
    fn native_pop(&mut self, builder: &mut FunctionBuilder) -> Result<Value, String> {
        use cranelift::prelude::*;
        
        let stack_ptr_slot = self.stack_ptr_slot.ok_or("Native stack not initialized")?;
        const VAR_SIZE: i32 = 8;
        
        // Load current stack pointer
        let stack_ptr = builder.ins().stack_load(types::I64, stack_ptr_slot, 0);
        
        // Decrement stack pointer
        let new_stack_ptr = builder.ins().iadd_imm(stack_ptr, -(VAR_SIZE as i64));
        builder.ins().stack_store(new_stack_ptr, stack_ptr_slot, 0);
        
        // Load value from decremented stack pointer
        let value = builder.ins().load(types::I64, MemFlags::new(), new_stack_ptr, 0);
        
        Ok(value)
    }
    
    /// Compile a sequence of bytecode to optimized machine code
    fn compile_sequence(&mut self, builder: &mut FunctionBuilder, code: &[Opcode]) -> Result<Value, String> {
        // Check if this sequence contains jumps - if so, use jump-aware compilation with native stack
        let has_jumps = code.iter().any(|op| matches!(op, Opcode::Jump(_) | Opcode::JumpIf(_) | Opcode::JumpIfNot(_) | Opcode::Label(_)));
        
        if has_jumps {
            return self.compile_sequence_with_jumps_native(builder, code);
        }
        
        // Look for optimization patterns first
        if let Some(result) = self.try_compile_arithmetic_sequence(builder, code)? {
            return Ok(result);
        }
        
        if let Some(result) = self.try_compile_constant_sequence(builder, code)? {
            return Ok(result);
        }
        
        // Fall back to general compilation
        self.compile_general_sequence(builder, code)
    }
    
    /// Try to compile a pure arithmetic sequence like [LoadConst(1), LoadConst(2), Add, Return]
    fn try_compile_arithmetic_sequence(&mut self, builder: &mut FunctionBuilder, code: &[Opcode]) -> Result<Option<Value>, String> {
        // Pattern: constants + arithmetic operations + return
        if code.len() < 3 {
            return Ok(None);
        }
        
        // Check if this is a simple arithmetic expression that we can constant-fold
        if let [Opcode::LoadConst(a), Opcode::LoadConst(b), Opcode::Add, Opcode::Return] = code {
            // Compile-time constant folding!
            if let (Some(a_int), Some(b_int)) = (a.as_int(), b.as_int()) {
                let result = a_int + b_int;
                let result_val = builder.ins().iconst(types::I64, result as i64);
                let result_var = self.var_builder.make_int(builder, result_val);
                return Ok(Some(result_var));
            }
        }
        
        if let [Opcode::LoadConst(a), Opcode::LoadConst(b), Opcode::Sub, Opcode::Return] = code {
            if let (Some(a_int), Some(b_int)) = (a.as_int(), b.as_int()) {
                let result = a_int - b_int;
                let result_val = builder.ins().iconst(types::I64, result as i64);
                let result_var = self.var_builder.make_int(builder, result_val);
                return Ok(Some(result_var));
            }
        }
        
        if let [Opcode::LoadConst(a), Opcode::LoadConst(b), Opcode::Less, Opcode::Return] = code {
            if let (Some(a_int), Some(b_int)) = (a.as_int(), b.as_int()) {
                let result = a_int < b_int;
                let result_bool = builder.ins().iconst(types::I8, if result { 1 } else { 0 });
                let result_var = self.var_builder.make_bool(builder, result_bool);
                return Ok(Some(result_var));
            }
        }
        
        Ok(None)
    }
    
    /// Try to compile a constant-only sequence
    fn try_compile_constant_sequence(&mut self, builder: &mut FunctionBuilder, code: &[Opcode]) -> Result<Option<Value>, String> {
        if let [Opcode::LoadConst(value), Opcode::Return] = code {
            // Direct constant load - no stack operations needed
            let const_val = builder.ins().iconst(types::I64, value.as_u64() as i64);
            return Ok(Some(const_val));
        }
        
        Ok(None)
    }
    
    
    

    /// Compile a sequence with jumps using native stack operations (replaces Vec<Value> approach)
    fn compile_sequence_with_jumps_native(&mut self, builder: &mut FunctionBuilder, code: &[Opcode]) -> Result<Value, String> {
        // Initialize native stack for this function if not already initialized
        if self.stack_base.is_none() {
            self.init_native_stack(builder)?;
        }
        
        // Try to compile an if-else pattern first
        if let Some(result) = self.try_compile_if_pattern_native(builder, code)? {
            return Ok(result);
        }
        
        // Try other patterns (while loops, etc.) - for now fall back to general compilation
        // Don't call compile_general_sequence as it would double-initialize the stack
        // Instead, implement general native compilation here
        for opcode in code {
            match opcode {
                Opcode::Return => {
                    return self.native_pop(builder);
                }
                _ => {
                    self.compile_single_opcode_native(builder, opcode)?;
                }
            }
        }
        
        // Default return - pop from native stack or return none
        self.native_pop(builder)
    }
    
    /// Try to compile an if-else pattern using native stack operations
    fn try_compile_if_pattern_native(&mut self, builder: &mut FunctionBuilder, code: &[Opcode]) -> Result<Option<Value>, String> {
        // Look for if-else pattern: [condition opcodes...] JumpIfNot(else) [then opcodes...] Jump(end) Label(else) [else opcodes...] Label(end)
        
        // Find JumpIfNot, Jump, and Labels
        let mut jump_if_not_idx = None;
        let mut jump_idx = None;
        let mut else_label_idx = None;
        let mut end_label_idx = None;
        let mut else_label = None;
        let mut end_label = None;
        
        for (i, opcode) in code.iter().enumerate() {
            match opcode {
                Opcode::JumpIfNot(label) => {
                    if jump_if_not_idx.is_none() {
                        jump_if_not_idx = Some(i);
                        else_label = Some(*label);
                    }
                }
                Opcode::Jump(label) => {
                    if jump_idx.is_none() && jump_if_not_idx.is_some() {
                        jump_idx = Some(i);
                        end_label = Some(*label);
                    }
                }
                Opcode::Label(label) => {
                    if Some(*label) == else_label && else_label_idx.is_none() {
                        else_label_idx = Some(i);
                    } else if Some(*label) == end_label && end_label_idx.is_none() {
                        end_label_idx = Some(i);
                    }
                }
                _ => {}
            }
        }
        
        // Verify we have a complete if-else pattern
        let (jump_if_not_idx, jump_idx, else_label_idx, end_label_idx) = match (jump_if_not_idx, jump_idx, else_label_idx, end_label_idx) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return Ok(None), // Not an if-else pattern
        };
        
        // Create blocks
        let then_block = builder.create_block();
        let else_block = builder.create_block();
        let end_block = builder.create_block();
        
        // Add a parameter to end_block to receive the result value
        builder.append_block_param(end_block, types::I64);
        
        // Compile condition using native stack operations
        for opcode in &code[0..jump_if_not_idx] {
            self.compile_single_opcode_native(builder, opcode)?;
        }
        
        // Pop condition from native stack
        let condition = self.native_pop(builder)?;
        
        // Branch based on condition
        let is_truthy = self.var_builder.emit_is_truthy(builder, condition);
        let is_truthy_i8 = builder.ins().ireduce(types::I8, is_truthy);
        builder.ins().brif(is_truthy_i8, then_block, &[], else_block, &[]);
        
        // Compile then branch
        builder.switch_to_block(then_block);
        builder.seal_block(then_block);
        for opcode in &code[jump_if_not_idx + 1..jump_idx] {
            self.compile_single_opcode_native(builder, opcode)?;
        }
        let then_result = self.native_pop(builder)?;
        builder.ins().jump(end_block, &[then_result]);
        
        // Compile else branch
        builder.switch_to_block(else_block);
        builder.seal_block(else_block);
        for opcode in &code[else_label_idx + 1..end_label_idx] {
            self.compile_single_opcode_native(builder, opcode)?;
        }
        let else_result = self.native_pop(builder)?;
        builder.ins().jump(end_block, &[else_result]);
        
        // End block
        builder.switch_to_block(end_block);
        builder.seal_block(end_block);
        
        // Return the result parameter
        Ok(Some(builder.block_params(end_block)[0]))
    }
    

    /// Compile a general sequence using stack simulation (fallback)
    fn compile_general_sequence(&mut self, builder: &mut FunctionBuilder, code: &[Opcode]) -> Result<Value, String> {
        // Initialize native stack for this function if not already initialized
        if self.stack_base.is_none() {
            self.init_native_stack(builder)?;
        }
        
        for opcode in code {
            match opcode {
                Opcode::Return => {
                    // Pop result from native stack
                    return self.native_pop(builder);
                }
                
                _ => {
                    self.compile_single_opcode_native(builder, opcode)?;
                }
            }
        }
        
        // Default return - pop from native stack or return none
        if self.stack_ptr_slot.is_some() {
            // Check if stack has values by comparing stack pointer to base
            let stack_ptr = builder.ins().stack_load(cranelift::prelude::types::I64, self.stack_ptr_slot.unwrap(), 0);
            let stack_base = self.stack_base.unwrap();
            let has_values = builder.ins().icmp(cranelift::prelude::IntCC::NotEqual, stack_ptr, stack_base);
            
            // If has values, pop; otherwise return none
            let then_block = builder.create_block();
            let else_block = builder.create_block();
            let merge_block = builder.create_block();
            
            // Add block parameter for the result
            builder.append_block_param(merge_block, cranelift::prelude::types::I64);
            
            builder.ins().brif(has_values, then_block, &[], else_block, &[]);
            
            // Then: pop value
            builder.switch_to_block(then_block);
            builder.seal_block(then_block);
            let popped_value = self.native_pop(builder)?;
            builder.ins().jump(merge_block, &[popped_value]);
            
            // Else: return none
            builder.switch_to_block(else_block);
            builder.seal_block(else_block);
            let none_value = self.var_builder.make_none(builder);
            builder.ins().jump(merge_block, &[none_value]);
            
            // Merge
            builder.switch_to_block(merge_block);
            builder.seal_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        } else {
            Ok(self.var_builder.make_none(builder))
        }
    }
    
    /// Compile a single opcode using native stack operations (replaces Vec<Value> stack)
    fn compile_single_opcode_native(&mut self, builder: &mut FunctionBuilder, opcode: &Opcode) -> Result<(), String> {
        match opcode {
            Opcode::LoadConst(var) => {
                // Load the constant as a full Var (64-bit value)
                let var_value = builder.ins().iconst(types::I64, var.as_u64() as i64);
                self.native_push(builder, var_value)?;
            }
            
            Opcode::LoadVar(symbol) => {
                if let Some(&value) = self.variables.get(symbol) {
                    // Found in local variables
                    self.native_push(builder, value)?;
                } else if let (Some(jit_ptr), Some(get_global_ref)) = (self.jit_ptr, self.get_global_ref) {
                    // Try global lookup via runtime helper
                    let symbol_id = builder.ins().iconst(types::I64, symbol.id() as i64);
                    let call_inst = builder.ins().call(get_global_ref, &[jit_ptr, symbol_id]);
                    let global_value = builder.inst_results(call_inst)[0];
                    self.native_push(builder, global_value)?;
                } else {
                    return Err(format!("Undefined variable: {symbol:?}"));
                }
            }
            
            Opcode::StoreVar(symbol) => {
                let value = self.native_pop(builder)?;
                self.variables.insert(*symbol, value);
                if let Some(current_scope) = self.scope_stack.last_mut() {
                    current_scope.push(*symbol);
                }
            }
            
            Opcode::DefGlobal(symbol) => {
                let value = self.native_pop(builder)?;
                if let (Some(jit_ptr), Some(set_global_ref)) = (self.jit_ptr, self.set_global_ref) {
                    // Call runtime helper to set global variable
                    let symbol_id = builder.ins().iconst(types::I64, symbol.id() as i64);
                    builder.ins().call(set_global_ref, &[jit_ptr, symbol_id, value]);
                } else {
                    return Err("DefGlobal requires JIT context".to_string());
                }
            }
            
            Opcode::PushScope(var_count) => {
                self.scope_stack.push(Vec::with_capacity(*var_count as usize));
            }
            
            Opcode::PopScope => {
                if let Some(scope_vars) = self.scope_stack.pop() {
                    for var_symbol in scope_vars {
                        self.variables.remove(&var_symbol);
                    }
                }
            }
            
            // Binary arithmetic operations - now using native stack
            Opcode::Add => {
                let b = self.native_pop(builder)?;
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_arithmetic_add(builder, a, b);
                self.native_push(builder, result)?;
            }
            
            Opcode::Sub => {
                let b = self.native_pop(builder)?;
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_arithmetic_sub(builder, a, b);
                self.native_push(builder, result)?;
            }
            
            Opcode::Mul => {
                let b = self.native_pop(builder)?;
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_arithmetic_mul(builder, a, b);
                self.native_push(builder, result)?;
            }
            
            Opcode::Div => {
                let b = self.native_pop(builder)?;
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_arithmetic_div(builder, a, b);
                self.native_push(builder, result)?;
            }
            
            Opcode::Mod => {
                let b = self.native_pop(builder)?;
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_arithmetic_mod(builder, a, b);
                self.native_push(builder, result)?;
            }
            
            // Comparison operations - native stack versions
            Opcode::Less => {
                let b = self.native_pop(builder)?;
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_arithmetic_lt(builder, a, b);
                self.native_push(builder, result)?;
            }
            
            // TODO: Implement the missing comparison operations
            // Opcode::Greater => {
            //     let b = self.native_pop(builder)?;
            //     let a = self.native_pop(builder)?;
            //     let result = self.var_builder.emit_arithmetic_gt(builder, a, b);
            //     self.native_push(builder, result)?;
            // }
            
            // Opcode::LessEqual => {
            //     let b = self.native_pop(builder)?;
            //     let a = self.native_pop(builder)?;
            //     let result = self.var_builder.emit_arithmetic_le(builder, a, b);
            //     self.native_push(builder, result)?;
            // }
            
            // Opcode::GreaterEqual => {
            //     let b = self.native_pop(builder)?;
            //     let a = self.native_pop(builder)?;
            //     let result = self.var_builder.emit_arithmetic_ge(builder, a, b);
            //     self.native_push(builder, result)?;
            // }
            
            // Opcode::Equal => {
            //     let b = self.native_pop(builder)?;
            //     let a = self.native_pop(builder)?;
            //     let result = self.var_builder.emit_arithmetic_eq(builder, a, b);
            //     self.native_push(builder, result)?;
            // }
            
            // Opcode::NotEqual => {
            //     let b = self.native_pop(builder)?;
            //     let a = self.native_pop(builder)?;
            //     let result = self.var_builder.emit_arithmetic_ne(builder, a, b);
            //     self.native_push(builder, result)?;
            // }

            // Logical operations
            Opcode::Not => {
                let a = self.native_pop(builder)?;
                let result = self.var_builder.emit_logical_not(builder, a);
                self.native_push(builder, result)?;
            }
            
            // Conditional selection
            Opcode::Select => {
                let else_val = self.native_pop(builder)?;
                let then_val = self.native_pop(builder)?;
                let condition = self.native_pop(builder)?;
                
                let is_truthy = self.var_builder.emit_is_truthy(builder, condition);
                let result = builder.ins().select(is_truthy, then_val, else_val);
                self.native_push(builder, result)?;
            }
            
            // Function calls - native stack version
            Opcode::Call(arg_count) => {
                // Pop arguments from native stack
                let mut args = Vec::with_capacity(*arg_count as usize);
                for _ in 0..*arg_count {
                    args.push(self.native_pop(builder)?);
                }
                args.reverse(); // Arguments were pushed in reverse order
                
                let function = self.native_pop(builder)?;
                
                // Call the jit_call_closure runtime helper
                let jit_ptr = self.jit_ptr.ok_or("No JIT pointer available for closure calls")?;
                
                // Prepare arguments array on stack
                let (args_ptr, arg_count_val) = if args.is_empty() {
                    (builder.ins().iconst(types::I64, 0), builder.ins().iconst(types::I32, 0))
                } else {
                    // Create stack slot for arguments
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (args.len() * 8) as u32,
                    ));
                    let args_addr = builder.ins().stack_addr(types::I64, slot, 0);
                    
                    // Store each argument
                    for (i, &arg) in args.iter().enumerate() {
                        builder.ins().store(MemFlags::trusted(), arg, args_addr, (i * 8) as i32);
                    }
                    
                    (args_addr, builder.ins().iconst(types::I32, args.len() as i64))
                };
                
                // Import the signature first
                let sig = builder.import_signature(Signature {
                    params: vec![
                        AbiParam::new(types::I64), // jit_ptr
                        AbiParam::new(types::I64), // closure
                        AbiParam::new(types::I64), // args_ptr
                        AbiParam::new(types::I32), // arg_count
                    ],
                    returns: vec![AbiParam::new(types::I64)], // return value
                    call_conv: CallConv::SystemV,
                });
                
                // Get the function pointer as a constant
                let func_ptr = builder.ins().iconst(types::I64, jit_call_closure as *const u8 as i64);
                
                // Create the call instruction
                let call_inst = builder.ins().call_indirect(
                    sig,
                    func_ptr,
                    &[jit_ptr, function, args_ptr, arg_count_val]
                );
                
                let result = builder.inst_results(call_inst)[0];
                
                self.native_push(builder, result)?;
            }
            
            // Closure creation - native stack version
            Opcode::Closure(function_id, _upvalue_count) => {
                // Look up lambda information in registry to get arity
                if let Some(registry) = &self.lambda_registry {
                    if let Some((params, _body)) = registry.get(function_id) {
                        let arity = params.len() as u32;
                        
                        // For now, create closure with null function pointer
                        // Lambda compilation will happen on-demand when the function is called
                        let closure_ptr = crate::heap::LispClosure::new(
                            std::ptr::null(), // Function pointer - will be compiled when needed
                            arity,
                            *function_id as u64  // Store function_id in captured_env for lazy compilation
                        );
                        
                        let closure_var = crate::var::Var::closure(closure_ptr);
                        let closure_value = builder.ins().iconst(types::I64, closure_var.as_u64() as i64);
                        self.native_push(builder, closure_value)?;
                    } else {
                        return Err(format!("Lambda function {function_id} not found in registry"));
                    }
                } else {
                    return Err("No lambda registry available for Closure opcode".to_string());
                }
            }
            
            // Return handled separately in compile_general_sequence
            Opcode::Return => {
                // No-op here, handled by caller
            }
            
            // Jumps and labels should not be reached in general sequence compilation
            Opcode::Jump(_) | Opcode::JumpIf(_) | Opcode::JumpIfNot(_) | Opcode::Label(_) => {
                return Err("Jump instructions should be handled by compile_sequence_with_jumps".to_string());
            }
            
            // Other opcodes that don't have native implementations yet
            _ => {
                return Err(format!("Native stack compilation for opcode {opcode:?} not yet implemented"));
            }
        }
        
        Ok(())
    }
    
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::Symbol;
    
    #[test]
    fn test_constant_folding_optimization() {
        let mut compiler = BytecodeCompiler::new();
        let expr = Expr::Call {
            func: Box::new(Expr::Variable(Symbol::mk("+"))),
            args: vec![
                Expr::Literal(Var::int(1)),
                Expr::Literal(Var::int(2)),
            ],
        };
        
        // Compile to bytecode
        let function = compiler.compile_expr(&expr).unwrap();
        
        // JIT compile with optimizations
        let mut jit = BytecodeJIT::new();
        let machine_code = jit.compile_function(&function, &compiler.lambda_registry).unwrap();
        
        // Execute the compiled function
        let func: fn() -> u64 = unsafe { std::mem::transmute(machine_code) };
        let result_bits = func();
        let result = Var::from_u64(result_bits);
        
        // Should compute 1 + 2 = 3, potentially constant-folded at compile time
        assert_eq!(result.as_int(), Some(3));
    }
}