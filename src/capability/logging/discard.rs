use core::convert::Infallible;

/// A logging capability, which discards all logging statements
pub struct Logging;

impl super::Logging for Logging {
    type Error = Infallible;

    fn debug(&self, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    fn info(&self, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    fn warn(&self, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    fn error(&self, _: String) -> Result<(), Self::Error> {
        Ok(())
    }
}
