//! Type protocol system for uniform operation dispatch across all Var types.
//! Inspired by Janet's JanetAbstractType system.

use crate::var::{Var, VarType};
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// A unified protocol for type operations, inspired by Janet's JanetAbstractType.
/// This allows both Rust code and JIT code to dispatch operations uniformly.
pub struct TypeProtocol {
    /// Convert value to string representation
    pub to_string: fn(*const ()) -> String,
    
    /// Hash the value for use in hash tables
    pub hash: fn(*const ()) -> u64,
    
    /// Check equality between two values of the same type
    pub equals: fn(*const (), *const ()) -> bool,
    
    /// Compare two values of the same type (-1, 0, 1)
    pub compare: fn(*const (), *const ()) -> i32,
    
    /// Get the length/size of the value (lists, strings, etc.)
    pub length: Option<fn(*const ()) -> i32>,
    
    /// Get an item by index/key (lists[index], maps[key])
    pub get: Option<fn(*const (), Var) -> Var>,
    
    /// Set an item by index/key (mutable operations)
    pub put: Option<fn(*mut (), Var, Var)>,
    
    /// Get next item for iteration
    pub next: Option<fn(*const (), Var) -> Var>,
    
    /// Call the value as a function
    pub call: Option<fn(*const (), &[Var]) -> Var>,
    
    /// Check if the value is truthy
    pub is_truthy: fn(*const ()) -> bool,
    
    /// Clone/copy the value
    pub clone: fn(*const ()) -> *mut (),
    
    /// Drop/free the value
    pub drop: fn(*mut ()),
}

/// Get the type protocol for a given VarType
pub fn get_protocol(var_type: VarType) -> &'static TypeProtocol {
    match var_type {
        VarType::None => &NONE_PROTOCOL,
        VarType::Bool => &BOOL_PROTOCOL,
        VarType::I32 => &I32_PROTOCOL,
        VarType::F64 => &F64_PROTOCOL,
        VarType::Symbol => &SYMBOL_PROTOCOL,
        VarType::List => &LIST_PROTOCOL,
        VarType::String => &STRING_PROTOCOL,
        VarType::Pointer => &POINTER_PROTOCOL,
    }
}

// Protocol implementations for each type
static NONE_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |_| "none".to_string(),
    hash: |_| 0, // None always hashes to 0
    equals: |_, _| true, // All None values are equal
    compare: |_, _| 0, // All None values are equal
    length: None, // None has no length
    get: None, // None is not indexable
    put: None, // None is not mutable
    next: None, // None is not iterable
    call: None, // None is not callable
    is_truthy: |_| false, // None is always falsy
    clone: |_| std::ptr::null_mut(), // None doesn't need cloning
    drop: |_| {}, // None doesn't need dropping
};

static BOOL_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        format!("{}", var.as_bool().unwrap())
    },
    hash: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        if var.as_bool().unwrap() { 1 } else { 0 }
    },
    equals: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        lvar.as_bool() == rvar.as_bool()
    },
    compare: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        match (lvar.as_bool().unwrap(), rvar.as_bool().unwrap()) {
            (false, true) => -1,
            (true, false) => 1,
            _ => 0,
        }
    },
    length: None,
    get: None,
    put: None,
    next: None,
    call: None,
    is_truthy: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_bool().unwrap()
    },
    clone: |ptr| ptr as *mut (),
    drop: |_| {},
};

static I32_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        format!("{}", var.as_int().unwrap())
    },
    hash: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_int().unwrap() as u64
    },
    equals: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        lvar.as_int() == rvar.as_int()
    },
    compare: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        let l = lvar.as_int().unwrap();
        let r = rvar.as_int().unwrap();
        if l < r { -1 } else if l > r { 1 } else { 0 }
    },
    length: None,
    get: None,
    put: None,
    next: None,
    call: None,
    is_truthy: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_int().unwrap() != 0
    },
    clone: |ptr| ptr as *mut (),
    drop: |_| {},
};

static F64_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        format!("{}", var.as_double().unwrap())
    },
    hash: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_double().unwrap().to_bits()
    },
    equals: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        lvar.as_double() == rvar.as_double()
    },
    compare: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        let l = lvar.as_double().unwrap();
        let r = rvar.as_double().unwrap();
        l.partial_cmp(&r).map_or(0, |ord| match ord {
            Ordering::Less => -1,
            Ordering::Greater => 1,
            Ordering::Equal => 0,
        })
    },
    length: None,
    get: None,
    put: None,
    next: None,
    call: None,
    is_truthy: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        let val = var.as_double().unwrap();
        val != 0.0 && !val.is_nan()
    },
    clone: |ptr| ptr as *mut (),
    drop: |_| {},
};

static SYMBOL_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        if let Some(sym) = var.as_symbol_obj() {
            sym.as_string().to_string()
        } else {
            format!("sym({})", var.as_symbol().unwrap())
        }
    },
    hash: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_symbol().unwrap() as u64
    },
    equals: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        lvar.as_symbol() == rvar.as_symbol()
    },
    compare: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        let l = lvar.as_symbol().unwrap();
        let r = rvar.as_symbol().unwrap();
        if l < r { -1 } else if l > r { 1 } else { 0 }
    },
    length: None,
    get: None,
    put: None, 
    next: None,
    call: None,
    is_truthy: |_| true, // Symbols are always truthy
    clone: |ptr| ptr as *mut (),
    drop: |_| {},
};

static LIST_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        let list = var.as_list().unwrap();
        let mut result = String::from("[");
        for (i, item) in list.iter().enumerate() {
            if i > 0 { result.push_str(", "); }
            result.push_str(&format!("{item}"));
        }
        result.push(']');
        result
    },
    hash: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        let list = var.as_list().unwrap();
        // Simple hash combining list length and first few elements
        let mut hash = list.len() as u64;
        for (i, item) in list.iter().take(3).enumerate() {
            hash ^= item.as_u64().wrapping_mul(i as u64 + 1);
        }
        hash
    },
    equals: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        lvar.as_list() == rvar.as_list()
    },
    compare: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        let l_list = lvar.as_list().unwrap();
        let r_list = rvar.as_list().unwrap();
        match l_list.len().cmp(&r_list.len()) {
            Ordering::Less => -1,
            Ordering::Greater => 1,
            Ordering::Equal => 0, // Could do lexicographic comparison here
        }
    },
    length: Some(|ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_list().unwrap().len() as i32
    }),
    get: Some(|ptr, index| {
        let var = unsafe { &*(ptr as *const Var) };
        let list = var.as_list().unwrap();
        if let Some(idx) = index.as_int() {
            if idx >= 0 && (idx as usize) < list.len() {
                return list[idx as usize];
            }
        }
        Var::none()
    }),
    put: None, // Lists are immutable in our implementation
    next: None, // TODO: Implement iteration
    call: None, // Lists are not callable
    is_truthy: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        !var.as_list().unwrap().is_empty()
    },
    clone: |ptr| ptr as *mut (), // Lists are immutable, can share
    drop: |_| {}, // Memory management handled by Rust
};

static STRING_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_string().unwrap().to_string()
    },
    hash: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        let s = var.as_string().unwrap();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    },
    equals: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        lvar.as_string() == rvar.as_string()
    },
    compare: |lhs, rhs| {
        let lvar = unsafe { &*(lhs as *const Var) };
        let rvar = unsafe { &*(rhs as *const Var) };
        let l_str = lvar.as_string().unwrap();
        let r_str = rvar.as_string().unwrap();
        match l_str.cmp(r_str) {
            Ordering::Less => -1,
            Ordering::Greater => 1,
            Ordering::Equal => 0,
        }
    },
    length: Some(|ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        var.as_string().unwrap().len() as i32
    }),
    get: Some(|ptr, index| {
        let var = unsafe { &*(ptr as *const Var) };
        let s = var.as_string().unwrap();
        if let Some(idx) = index.as_int() {
            if idx >= 0 && (idx as usize) < s.len() {
                if let Some(ch) = s.chars().nth(idx as usize) {
                    return Var::string(&ch.to_string());
                }
            }
        }
        Var::none()
    }),
    put: None, // Strings are immutable
    next: None, // TODO: Implement iteration
    call: None, // Strings are not callable
    is_truthy: |ptr| {
        let var = unsafe { &*(ptr as *const Var) };
        !var.as_string().unwrap().is_empty()
    },
    clone: |ptr| ptr as *mut (), // Strings are immutable, can share
    drop: |_| {}, // Memory management handled by Rust
};

static POINTER_PROTOCOL: TypeProtocol = TypeProtocol {
    to_string: |ptr| {
        format!("ptr({:p})", ptr)
    },
    hash: |ptr| ptr as u64,
    equals: |lhs, rhs| lhs == rhs,
    compare: |lhs, rhs| {
        if lhs < rhs { -1 } else if lhs > rhs { 1 } else { 0 }
    },
    length: None,
    get: None,
    put: None,
    next: None,
    call: None,
    is_truthy: |_| true, // Pointers are always truthy
    clone: |ptr| ptr as *mut (),
    drop: |_| {},
};