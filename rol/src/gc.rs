//! Garbage collection integration for heap-allocated types.
//! Provides root enumeration and child tracing for GC systems like mmtk.

use crate::var::Var;
use crate::heap::{LispString, LispVector};

/// Trait for objects that can be traced by the garbage collector.
/// All heap-allocated types must implement this.
pub trait GcTrace {
    /// Visit all GC-managed references contained within this object.
    /// The visitor function should be called for each Var that contains a heap reference.
    fn trace_children<F>(&self, visitor: F) 
    where F: FnMut(&Var);
    
    /// Get the size of this object in bytes for GC accounting.
    fn size_bytes(&self) -> usize;
    
    /// Get the type name for debugging/introspection.
    fn type_name(&self) -> &'static str;
}

/// Root set enumeration for garbage collection.
/// The GC will call this to find all roots from which to start tracing.
pub trait GcRootSet {
    /// Visit all root references that should not be collected.
    /// This includes stack variables, global variables, JIT live values, etc.
    fn trace_roots<F>(&self, visitor: F)
    where F: FnMut(&Var);
}

impl GcTrace for LispString {
    fn trace_children<F>(&self, _visitor: F) 
    where F: FnMut(&Var) 
    {
        // Strings have no child references - they're leaf objects
    }
    
    fn size_bytes(&self) -> usize {
        std::mem::size_of::<LispString>() + self.length as usize
    }
    
    fn type_name(&self) -> &'static str {
        "LispString"
    }
}

impl GcTrace for LispVector {
    fn trace_children<F>(&self, mut visitor: F) 
    where F: FnMut(&Var) 
    {
        // Visit all elements in the vector - they might contain heap references
        unsafe {
            let elements = self.as_slice();
            for element in elements {
                visitor(element);
            }
        }
    }
    
    fn size_bytes(&self) -> usize {
        std::mem::size_of::<LispVector>() + 
        (self.capacity as usize * std::mem::size_of::<Var>())
    }
    
    fn type_name(&self) -> &'static str {
        "LispVector"
    }
}

/// Determine if a Var contains a heap reference that needs GC tracing.
pub fn var_needs_tracing(var: &Var) -> bool {
    var.is_list() || var.is_string()
    // Could extend to other heap types: symbols, closures, etc.
}

/// Extract heap object from a Var for GC tracing.
/// Returns None if the Var doesn't contain a traceable heap reference.
pub unsafe fn var_as_gc_object(var: &Var) -> Option<GcObjectRef> {
    if var.is_list() {
        let ptr_bits = var.as_u64() & !crate::var::POINTER_TAG_MASK;
        let ptr = ptr_bits as *const LispVector;
        Some(GcObjectRef::Vector(ptr))
    } else if var.is_string() {
        let ptr_bits = var.as_u64() & !crate::var::POINTER_TAG_MASK;
        let ptr = ptr_bits as *const LispString;
        Some(GcObjectRef::String(ptr))
    } else {
        None
    }
}

/// Type-erased reference to a GC-managed heap object.
pub enum GcObjectRef {
    String(*const LispString),
    Vector(*const LispVector),
}

impl GcObjectRef {
    /// Trace children of this object, regardless of concrete type.
    pub unsafe fn trace_children<F>(&self, visitor: F)
    where F: FnMut(&Var)
    {
        match self {
            GcObjectRef::String(ptr) => (*ptr).as_ref().unwrap().trace_children(visitor),
            GcObjectRef::Vector(ptr) => (*ptr).as_ref().unwrap().trace_children(visitor),
        }
    }
    
    /// Get size in bytes for GC accounting.
    pub unsafe fn size_bytes(&self) -> usize {
        match self {
            GcObjectRef::String(ptr) => (*ptr).as_ref().unwrap().size_bytes(),
            GcObjectRef::Vector(ptr) => (*ptr).as_ref().unwrap().size_bytes(),
        }
    }
    
    /// Get type name for debugging.
    pub unsafe fn type_name(&self) -> &'static str {
        match self {
            GcObjectRef::String(ptr) => (*ptr).as_ref().unwrap().type_name(),
            GcObjectRef::Vector(ptr) => (*ptr).as_ref().unwrap().type_name(),
        }
    }
}

/// Example GC root set implementation for a simple runtime.
pub struct SimpleRootSet {
    /// Stack-allocated Vars
    pub stack_vars: Vec<Var>,
    /// Global variables  
    pub globals: Vec<Var>,
    /// JIT live values (would be more complex in practice)
    pub jit_live: Vec<Var>,
}

impl GcRootSet for SimpleRootSet {
    fn trace_roots<F>(&self, mut visitor: F)
    where F: FnMut(&Var)
    {
        // Visit all stack variables
        for var in &self.stack_vars {
            visitor(var);
        }
        
        // Visit all global variables
        for var in &self.globals {
            visitor(var);
        }
        
        // Visit all JIT live values
        for var in &self.jit_live {
            visitor(var);
        }
    }
}

/// Complete GC tracing starting from a root set.
/// This would be called by the actual GC implementation.
pub unsafe fn trace_from_roots<R: GcRootSet>(
    roots: &R, 
    mut mark_object: impl FnMut(*const u8)
) {
    let mut worklist = Vec::new();
    
    // Add all roots to worklist
    roots.trace_roots(|var| {
        if var_needs_tracing(var) {
            worklist.push(*var);
        }
    });
    
    // Process worklist until empty
    while let Some(var) = worklist.pop() {
        if let Some(obj_ref) = var_as_gc_object(&var) {
            // Mark this object as reachable
            let ptr = match obj_ref {
                GcObjectRef::String(ptr) => ptr as *const u8,
                GcObjectRef::Vector(ptr) => ptr as *const u8,
            };
            mark_object(ptr);
            
            // Add children to worklist
            obj_ref.trace_children(|child_var| {
                if var_needs_tracing(child_var) {
                    worklist.push(*child_var);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_string_tracing() {
        let string_var = Var::string("hello");
        
        // String should not need child tracing (it's a leaf)
        unsafe {
            if let Some(obj_ref) = var_as_gc_object(&string_var) {
                let mut child_count = 0;
                obj_ref.trace_children(|_| child_count += 1);
                assert_eq!(child_count, 0, "Strings should have no children");
            }
        }
    }
    
    #[test] 
    fn test_vector_tracing() {
        let elements = [
            Var::int(42),
            Var::string("nested"),
            Var::bool(true)
        ];
        let list_var = Var::list(&elements);
        
        // Vector should trace all its elements
        unsafe {
            if let Some(obj_ref) = var_as_gc_object(&list_var) {
                let mut traced_vars = Vec::new();
                obj_ref.trace_children(|var| traced_vars.push(*var));
                
                assert_eq!(traced_vars.len(), 3);
                assert_eq!(traced_vars[0].as_int(), Some(42));
                assert_eq!(traced_vars[1].as_string(), Some("nested"));
                assert_eq!(traced_vars[2].as_bool(), Some(true));
            }
        }
    }
    
    #[test]
    fn test_nested_heap_objects() {
        // Create nested structure: list containing another list and string
        let inner_list = Var::list(&[Var::int(1), Var::int(2)]);
        let string = Var::string("test");
        let outer_list = Var::list(&[inner_list, string, Var::none()]);
        
        // Should trace all nested heap objects
        let mut root_set = SimpleRootSet {
            stack_vars: vec![outer_list],
            globals: vec![],
            jit_live: vec![],
        };
        
        let mut marked_objects = Vec::new();
        unsafe {
            trace_from_roots(&root_set, |ptr| {
                marked_objects.push(ptr);
            });
        }
        
        // Should mark outer list, inner list, and string
        assert_eq!(marked_objects.len(), 3);
    }
}