//! Escaped JSON fields formatter that provides a drop-in replacement for tracing-subscriber's JsonFields
//! but with proper JSON escaping for Debug output.

use serde_json;
use std::fmt::{self, Write};
use tracing_subscriber::field::{MakeVisitor, VisitFmt, VisitOutput, RecordFields};
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::fmt::format::Writer;

/// A field formatter that properly escapes JSON content, especially Debug output.
/// 
/// This is a transparent replacement for tracing-subscriber's JsonFields that fixes
/// the JSON escaping issue when using Debug formatting (`?field`).
/// 
/// ## Usage
/// 
/// ```rust
/// use tracing_json::EscapedJsonFields;
/// use tracing_subscriber::fmt;
/// 
/// let layer = fmt::layer()
///     .event_format(fmt::format().json())
///     .fmt_fields(EscapedJsonFields::new());  // Instead of JsonFields::new()
/// ```
#[derive(Debug, Clone)]
pub struct EscapedJsonFields;

impl EscapedJsonFields {
    /// Create a new `EscapedJsonFields` formatter.
    pub fn new() -> Self {
        Self
    }
}

impl Default for EscapedJsonFields {
    fn default() -> Self {
        Self::new()
    }
}

impl<'writer> FormatFields<'writer> for EscapedJsonFields {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = EscapedJsonVisitor::new(&mut writer);
        fields.record(&mut visitor);
        visitor.finish()
    }
}

impl<'writer> MakeVisitor<&'writer mut dyn Write> for EscapedJsonFields {
    type Visitor = EscapedJsonVisitor<'writer>;

    fn make_visitor(&self, target: &'writer mut dyn Write) -> Self::Visitor {
        EscapedJsonVisitor::new(target)
    }
}

/// A visitor that properly escapes JSON content.
pub struct EscapedJsonVisitor<'writer> {
    writer: &'writer mut dyn Write,
    result: fmt::Result,
    is_first: bool,
}

impl<'writer> EscapedJsonVisitor<'writer> {
    fn new(writer: &'writer mut dyn Write) -> Self {
        Self {
            writer,
            result: Ok(()),
            is_first: true,
        }
    }

    fn maybe_write_comma(&mut self) -> fmt::Result {
        if self.is_first {
            self.is_first = false;
            Ok(())
        } else {
            write!(self.writer, ",")
        }
    }

    fn write_escaped_field(&mut self, field: &tracing::field::Field, value: &str) -> fmt::Result {
        self.maybe_write_comma()?;
        
        // Use serde_json to properly escape the field name and value
        // If serde_json fails, fall back to basic escaping
        let field_name = match serde_json::to_string(field.name()) {
            Ok(name) => name,
            Err(_) => format!("\"{}\"", field.name().replace('"', "\\\"")),
        };
        
        let escaped_value = match serde_json::to_string(value) {
            Ok(val) => val,
            Err(_) => format!("\"{}\"", value.replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")),
        };
        
        write!(self.writer, "{}:{}", field_name, escaped_value)
    }

    fn write_field_with_value<T: fmt::Display>(&mut self, field: &tracing::field::Field, value: T) -> fmt::Result {
        self.maybe_write_comma()?;
        
        let field_name = match serde_json::to_string(field.name()) {
            Ok(name) => name,
            Err(_) => format!("\"{}\"", field.name().replace('"', "\\\"")),
        };
        
        write!(self.writer, "{}:{}", field_name, value)
    }
}

impl<'writer> tracing::field::Visit for EscapedJsonVisitor<'writer> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if self.result.is_err() {
            return;
        }

        let debug_output = format!("{:?}", value);
        self.result = self.write_escaped_field(field, &debug_output);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_escaped_field(field, value);
    }

    fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
        if self.result.is_err() {
            return;
        }

        let hex_string = hex::encode(value);
        self.result = self.write_escaped_field(field, &hex_string);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_field_with_value(field, value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_field_with_value(field, value);
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_field_with_value(field, value);
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_field_with_value(field, value);
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_field_with_value(field, value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if self.result.is_err() {
            return;
        }

        self.result = self.write_field_with_value(field, value);
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        if self.result.is_err() {
            return;
        }

        let error_string = value.to_string();
        self.result = self.write_escaped_field(field, &error_string);
    }
}

impl<'writer> VisitOutput<fmt::Result> for EscapedJsonVisitor<'writer> {
    fn finish(self) -> fmt::Result {
        self.result
    }
}

impl<'writer> VisitFmt for EscapedJsonVisitor<'writer> {
    fn writer(&mut self) -> &mut dyn Write {
        self.writer
    }
}