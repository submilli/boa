//! Bound diagnostic formatting before output can amplify page data.
use boa_engine::{JsNativeError, JsResult, JsString};
use std::fmt::{self, Write};
struct Output {
    text: String,
    limit: usize,
}
impl Write for Output {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if s.len() > self.limit.saturating_sub(self.text.len()) {
            return Err(fmt::Error);
        }
        self.text.push_str(s);
        Ok(())
    }
}
pub(super) fn check(length: usize, limit: usize) -> JsResult<()> {
    if length > limit {
        return Err(JsNativeError::range()
            .with_message("Console output limit exceeded")
            .into());
    }
    Ok(())
}
pub(super) fn display(value: &impl fmt::Display, limit: usize) -> JsResult<String> {
    let mut output = Output {
        text: String::new(),
        limit,
    };
    write!(&mut output, "{value}")
        .map_err(|_| JsNativeError::range().with_message("Console output limit exceeded"))?;
    Ok(output.text)
}
pub(super) fn string(value: &JsString) -> JsResult<String> {
    check(value.len(), 65536)?;
    let text = value.to_std_string_escaped();
    check(text.len(), 65536)?;
    Ok(text)
}
