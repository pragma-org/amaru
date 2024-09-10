/// Generic trait for validating any type of data. Designed to be used across threads so validations
/// can be done in parallel. Validators are expected to never result in an error, but instead panic
/// if the validation is unable to be completed and return a true or false. This indicates a serious
/// bug in the code.
pub trait Validator: Send + Sync {
    fn validate(&self) -> bool;
}
