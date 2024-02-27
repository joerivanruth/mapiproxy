use std::io::{self, BufRead, Read};

/// A MyBufReader is like a regular BufReader except that you can pass it
/// some initial content at creation time.
pub struct MyBufReader<'a> {
    inner: Box<dyn io::Read + 'a>,
    buffer: Vec<u8>,
    data_start: usize,
    data_end: usize,
}

impl<'a> MyBufReader<'a> {
    /// Create a new MyBufReader. Reading from it will first return the current
    /// contents of the Vec before starting to read from the reader.
    /// The whole capacity of the Vec will be used as buffer space.
    pub fn new(reader: impl io::Read + 'a, mut buffer: Vec<u8>) -> Self {
        let inner = Box::new(reader);
        let data_end = buffer.len();
        buffer.resize(buffer.capacity(), 0u8);
        MyBufReader {
            inner,
            buffer,
            data_start: 0,
            data_end,
        }
    }
}

impl<'a> Read for MyBufReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let data = self.fill_buf()?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl<'a> BufRead for MyBufReader<'a> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        assert_eq!(self.buffer.len(), self.buffer.capacity());
        if self.data_start == self.data_end {
            let nread = self.inner.read(&mut self.buffer)?;
            self.data_start = 0;
            self.data_end = nread;
        }
        Ok(&self.buffer[self.data_start..self.data_end])
    }

    fn consume(&mut self, n: usize) {
        self.data_start += n;
    }
}
