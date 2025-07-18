// Import the main logic from main.rs
// We'll just include the main.rs file directly to avoid module complexity
include!("../main.rs");

// This binary will use the same main() function as dbcrust
// The only difference is the binary name, which can be detected at runtime if needed