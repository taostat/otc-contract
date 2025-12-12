/// Shared error types used by FixedDecimal and other common operations
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum SharedError {
    /// Division by zero error
    DivisionByZero,
    /// Arithmetic overflow
    Overflow,
}
