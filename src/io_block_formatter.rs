use bytes::{BufMut, Bytes, BytesMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockIoFormat {
    signature: u16,
}

impl BlockIoFormat {
    pub const fn new(signature: u16) -> Self {
        BlockIoFormat { signature }
    }

    pub fn format(&self, bytes: Bytes) -> Bytes {
        let capacity = bytes.len() + size_of::<u16>() + size_of::<u32>();
        let mut buffer = BytesMut::with_capacity(capacity);
        buffer.put_u16(self.signature);
        buffer.put_u32(bytes.len() as u32);
        buffer.put(bytes);
        buffer.into()
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Buf, Bytes};

    use crate::io_block_formatter::BlockIoFormat;

    #[test]
    fn format() {
        static MSG: &[u8] = b"hello world";

        let formatter = BlockIoFormat::new(0x1B69);
        let payload = Bytes::copy_from_slice(MSG);
        let mut formatted = formatter.format(payload);
        assert_eq!(
            formatted.len(),
            MSG.len() + size_of::<u16>() + size_of::<u32>()
        );

        let sig = formatted.get_u16();
        let len = formatted.get_u32();

        assert_eq!(sig, 0x1B69);
        assert_eq!(len, 11);
        assert_eq!(&formatted, MSG);
    }
}
