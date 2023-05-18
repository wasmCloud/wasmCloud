/// Converts error from Send + Sync error to standard anyhow error
pub(crate) fn boxed_err_to_anyhow(e: Box<dyn ::std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}
