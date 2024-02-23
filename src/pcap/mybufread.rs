use std::io::{self, BufRead, Read};

pub struct MyBufReader<'a> {
    inner: Box<dyn io::Read + 'a>,
    buffer: Vec<u8>,
    data: usize,
    free: usize,
}

impl<'a> MyBufReader<'a> {
    pub fn new(reader: impl io::Read + 'a, mut buffer: Vec<u8>) -> Self {
        let inner = Box::new(reader);
        let data = 0;
        let free = buffer.len();
        buffer.resize(buffer.capacity(), 0u8);
        MyBufReader {
            inner,
            buffer,
            data,
            free,
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
        if self.data == self.free {
            let nread = self.inner.read(&mut self.buffer)?;
            self.data = 0;
            self.free = nread;
        }
        Ok(&self.buffer[self.data..self.free])
    }

    fn consume(&mut self, n: usize) {
        self.data += n;
    }
}