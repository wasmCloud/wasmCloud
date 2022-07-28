use crate::BytesMut;

#[derive(Default)]
pub struct Writer {
    writer: BytesMut,
}

impl Writer {
    #[inline]
    pub fn write<B: ToBytes>(&mut self, bytes: B) {
        self.writer.extend_from_slice(bytes.to_bytes());
    }

    /// Returns the current buffer, zeroing out self
    pub fn take(&mut self) -> BytesMut {
        self.writer.split_to(self.writer.len())
    }

    /// Returns current position
    pub fn pos(&self) -> usize {
        self.writer.len()
    }

    /// Returns slice from writer
    pub fn get_slice(&self, start_pos: usize, end_pos: usize) -> &[u8] {
        &self.writer[start_pos..end_pos]
    }
}

pub trait ToBytes {
    fn to_bytes(&self) -> &[u8];
}
impl ToBytes for &str {
    fn to_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}
impl ToBytes for &String {
    fn to_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}
impl ToBytes for &[u8] {
    fn to_bytes(&self) -> &[u8] {
        self
    }
}
impl ToBytes for BytesMut {
    fn to_bytes(&self) -> &[u8] {
        self.as_ref()
    }
}

impl std::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.write(s.as_bytes());
        Ok(())
    }

    fn write_char(&mut self, c: char) -> std::fmt::Result {
        let mut chars = [0u8; 4];
        let s = c.encode_utf8(&mut chars);
        self.write(s.as_bytes());
        Ok(())
    }
}

impl<const N: usize> ToBytes for &[u8; N] {
    fn to_bytes(&self) -> &[u8] {
        self.as_slice()
    }
}
