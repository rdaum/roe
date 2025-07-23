//! Native heap-allocated types for Lisp runtime.
//! These types are designed for JIT access and will later integrate with mmtk GC.

use crate::var::Var;
use std::alloc::{alloc, dealloc, Layout};
use std::ptr;
use std::slice;
use std::str;

/// Native string type optimized for JIT access.
/// Layout: [length: u64][bytes: u8...]
/// String data follows immediately after length field.
#[repr(C)]
pub struct LispString {
    /// Length of the string in bytes
    pub length: u64,
    // String data follows immediately after length (flexible array member)
}

impl LispString {
    /// Create a new LispString from a Rust string slice
    pub fn from_str(s: &str) -> *mut LispString {
        let bytes = s.as_bytes();
        let length = bytes.len() as u64;
        
        // Calculate total size: header + string data
        let header_size = std::mem::size_of::<LispString>();
        let total_size = header_size + bytes.len();
        let align = std::mem::align_of::<LispString>();
        
        // Allocate memory
        let layout = Layout::from_size_align(total_size, align).unwrap();
        let ptr = unsafe { alloc(layout) as *mut LispString };
        
        if ptr.is_null() {
            panic!("Failed to allocate memory for LispString");
        }
        
        unsafe {
            // Initialize header
            (*ptr).length = length;
            
            // Copy string data immediately after header
            let data_ptr = (ptr as *mut u8).add(header_size);
            ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
        }
        
        ptr
    }
    
    /// Get the string data as a byte slice
    pub unsafe fn as_bytes(&self) -> &[u8] { unsafe {
        let data_ptr = (self as *const LispString as *const u8)
            .add(std::mem::size_of::<LispString>());
        slice::from_raw_parts(data_ptr, self.length as usize)
    }}
    
    /// Get the string data as a string slice  
    pub unsafe fn as_str(&self) -> &str {
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }
    
    /// Free the memory for this LispString
    pub unsafe fn free(ptr: *mut LispString) { unsafe {
        if ptr.is_null() {
            return;
        }
        
        let header_size = std::mem::size_of::<LispString>();
        let total_size = header_size + (*ptr).length as usize;
        let align = std::mem::align_of::<LispString>();
        
        let layout = Layout::from_size_align_unchecked(total_size, align);
        dealloc(ptr as *mut u8, layout);
    }}
}

/// Native vector type optimized for JIT access.
/// Layout: [length: u64][capacity: u64][elements: Var...]
/// Vector data follows immediately after capacity field.
#[repr(C)]
pub struct LispVector {
    /// Number of elements currently in the vector
    pub length: u64,
    /// Number of elements that can fit without reallocation
    pub capacity: u64,
    // Vector data follows immediately after capacity (flexible array member)
}

impl LispVector {
    /// Create a new empty LispVector with the given capacity
    pub fn with_capacity(capacity: usize) -> *mut LispVector {
        let capacity = capacity as u64;
        
        // Calculate total size: header + element storage
        let header_size = std::mem::size_of::<LispVector>();
        let elements_size = capacity as usize * std::mem::size_of::<Var>();
        let total_size = header_size + elements_size;
        let align = std::mem::align_of::<LispVector>();
        
        // Allocate memory
        let layout = Layout::from_size_align(total_size, align).unwrap();
        let ptr = unsafe { alloc(layout) as *mut LispVector };
        
        if ptr.is_null() {
            panic!("Failed to allocate memory for LispVector");
        }
        
        unsafe {
            // Initialize header
            (*ptr).length = 0;
            (*ptr).capacity = capacity;
            
            // Zero out element storage
            let data_ptr = (ptr as *mut u8).add(header_size);
            ptr::write_bytes(data_ptr, 0, elements_size);
        }
        
        ptr
    }
    
    /// Create a new LispVector from a slice of Vars
    pub fn from_slice(elements: &[Var]) -> *mut LispVector {
        let ptr = Self::with_capacity(elements.len());
        
        unsafe {
            (*ptr).length = elements.len() as u64;
            
            // Copy elements
            let data_ptr = Self::data_ptr(ptr);
            ptr::copy_nonoverlapping(elements.as_ptr(), data_ptr, elements.len());
        }
        
        ptr
    }
    
    /// Create a new empty LispVector
    pub fn new() -> *mut LispVector {
        Self::with_capacity(0)
    }
    
    /// Get pointer to the element data
    unsafe fn data_ptr(ptr: *mut LispVector) -> *mut Var {
        unsafe { (ptr as *mut u8).add(std::mem::size_of::<LispVector>()) as *mut Var }
    }
    
    /// Get the elements as a slice
    pub unsafe fn as_slice(&self) -> &[Var] { unsafe {
        let data_ptr = (self as *const LispVector as *mut u8)
            .add(std::mem::size_of::<LispVector>()) as *const Var;
        slice::from_raw_parts(data_ptr, self.length as usize)
    }}
    
    /// Get the elements as a mutable slice
    pub unsafe fn as_mut_slice(&mut self) -> &mut [Var] { unsafe {
        let data_ptr = (self as *mut LispVector as *mut u8)
            .add(std::mem::size_of::<LispVector>()) as *mut Var;
        slice::from_raw_parts_mut(data_ptr, self.length as usize)
    }}
    
    /// Push an element to the vector (may reallocate)
    pub unsafe fn push(ptr: *mut LispVector, element: Var) -> *mut LispVector { unsafe {
        let length = (*ptr).length;
        let capacity = (*ptr).capacity;
        
        if length < capacity {
            // Space available, just add element
            let data_ptr = Self::data_ptr(ptr);
            *data_ptr.add(length as usize) = element;
            (*ptr).length = length + 1;
            ptr
        } else {
            // Need to reallocate
            let new_capacity = if capacity == 0 { 4 } else { capacity * 2 };
            let new_ptr = Self::with_capacity(new_capacity as usize);
            
            // Copy existing elements
            if length > 0 {
                let old_data = Self::data_ptr(ptr);
                let new_data = Self::data_ptr(new_ptr);
                ptr::copy_nonoverlapping(old_data, new_data, length as usize);
            }
            
            // Add new element
            (*new_ptr).length = length + 1;
            let new_data = Self::data_ptr(new_ptr);
            *new_data.add(length as usize) = element;
            
            // Free old vector
            Self::free(ptr);
            
            new_ptr
        }
    }}
    
    /// Free the memory for this LispVector
    pub unsafe fn free(ptr: *mut LispVector) { unsafe {
        if ptr.is_null() {
            return;
        }
        
        let header_size = std::mem::size_of::<LispVector>();
        let elements_size = (*ptr).capacity as usize * std::mem::size_of::<Var>();
        let total_size = header_size + elements_size;
        let align = std::mem::align_of::<LispVector>();
        
        let layout = Layout::from_size_align_unchecked(total_size, align);
        dealloc(ptr as *mut u8, layout);
    }}
}

// For debugging
impl std::fmt::Debug for LispString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unsafe {
            write!(f, "LispString({:?})", self.as_str())
        }
    }
}

impl std::fmt::Debug for LispVector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unsafe {
            write!(f, "LispVector({:?})", self.as_slice())
        }
    }
}