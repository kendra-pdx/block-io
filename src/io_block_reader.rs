use bytes::{Buf, BufMut, Bytes, BytesMut};

#[derive(Debug, thiserror::Error)]
pub enum IoBlockReaderError {
    #[error("block was read, but not taken")]
    BlockNotTaken,
}

#[derive(Default)]
pub struct ReadState {
    buffer: BytesMut,
}

#[derive(strum::EnumTryAs, strum::EnumIs)]
pub enum IoBlockReader {
    /// initialize the block reader with the expected signature
    Init(u16),

    /// reading 2 bytes to match the signature
    /// will remain in this state until the tail of the buffer contains the `expect` value
    ReadingExpect { expect: u16, read_state: ReadState },

    /// reading 4 bytes to determine the data size
    ReadingBlockSize(ReadState),

    /// reading N bytes of data
    ReadingBlock {
        block_size: u32,
        read_state: ReadState,
    },

    /// read all of the data and now have a set of bytes to take
    Block(Bytes),

    /// something went wrong with reading
    Error(IoBlockReaderError),
}

impl ReadState {
    pub fn bytes_read(&self) -> usize {
        self.buffer.len()
    }

    fn push(&mut self, b: u8) {
        self.buffer.put_u8(b);
    }

    fn tail_u16(&self) -> Option<u16> {
        const SIZE: usize = size_of::<u16>();
        self.tail_n::<SIZE>().map(|mut bytes| bytes.get_u16())
    }

    fn tail_u32(&self) -> Option<u32> {
        const SIZE: usize = size_of::<u32>();
        self.tail_n::<SIZE>().map(|mut bytes| bytes.get_u32())
    }

    pub fn tail_n<const N: usize>(&self) -> Option<Bytes> {
        if self.bytes_read() >= N {
            let ix = self.buffer.len() - N;
            let bytes = Bytes::copy_from_slice(&self.buffer[ix..]);
            Some(bytes)
        } else {
            None
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buffer
    }
}

impl IoBlockReader {
    pub fn update(self, byte: u8) -> Self {
        match self {
            IoBlockReader::Init(expect) => Self::ReadingExpect {
                expect,
                read_state: ReadState::default(),
            }
            .update(byte),
            IoBlockReader::ReadingExpect {
                expect,
                mut read_state,
            } => {
                read_state.push(byte);
                if let Some(actual) = read_state.tail_u16()
                    && actual == expect
                {
                    Self::ReadingBlockSize(ReadState::default())
                } else {
                    Self::ReadingExpect { expect, read_state }
                }
            }
            Self::ReadingBlockSize(mut read_state) => {
                read_state.push(byte);
                if let Some(block_size) = read_state.tail_u32() {
                    Self::ReadingBlock {
                        block_size,
                        read_state: ReadState::default(),
                    }
                } else {
                    Self::ReadingBlockSize(read_state)
                }
            }
            Self::ReadingBlock {
                block_size,
                mut read_state,
            } => {
                read_state.push(byte);
                if read_state.bytes_read() as u32 == block_size {
                    let block = read_state.buffer.into();
                    Self::Block(block)
                } else {
                    Self::ReadingBlock {
                        block_size,
                        read_state,
                    }
                }
            }
            Self::Error(_) => self,
            Self::Block(_) => Self::Error(IoBlockReaderError::BlockNotTaken),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::addr_of;

    use bytes::{BufMut, Bytes, BytesMut};
    use rkyv::{Archive, Deserialize, Serialize};
    use rstest::{fixture, rstest};

    use crate::{BlockIoFormat, IoBlockReader, ReadState};

    #[test]
    fn read_state_tail() {
        let mut buffer = BytesMut::new();
        buffer.put_u16(0x4269);
        let state = ReadState { buffer };
        assert_eq!(Some(0x4269), state.tail_u16());

        let mut buffer = BytesMut::new();
        buffer.put_u32(0xDEAD_BEEF);
        let state = ReadState { buffer };
        assert_eq!(Some(0xDEAD_BEEF), state.tail_u32());

        let buffer = BytesMut::from_iter(b"hello world\n");
        let state = ReadState { buffer };
        assert_eq!(Some(Bytes::from_static(b"\n")), state.tail_n::<1>());

        assert_eq!(12, state.bytes().len());
    }

    #[rstest]
    fn io_block_reader(message_1: Record) {
        const SIG: u16 = 0x4269;
        const FMT: BlockIoFormat = BlockIoFormat::new(SIG);

        let bytes = message_1.to_bytes().unwrap();
        let buffer = FMT.format(bytes);

        let mut bytes = buffer.iter();

        let mut state = IoBlockReader::Init(SIG);

        while let Some(byte) = bytes.next()
            && !state.is_block()
        {
            state = state.update(*byte);
        }

        let block = state.try_as_block().unwrap();
        let message = Record::from_bytes(&block).unwrap();
        assert_eq!(message, message_1);
    }

    #[rstest]
    fn io_block_reader_block_update_block_not_taken(message_1: Record) {
        const SIG: u16 = 0x4269;
        const FMT: BlockIoFormat = BlockIoFormat::new(SIG);

        let bytes = message_1.to_bytes().unwrap();

        let buffer = FMT.format(bytes);
        let mut bytes = buffer.iter();

        let mut state = IoBlockReader::Init(0x4269);

        while let Some(byte) = bytes.next()
            && !state.is_block()
        {
            state = state.update(*byte);
        }

        assert!(state.is_block());

        state = state.update(b'?');
        assert!(state.is_error());

        state = state.update(b'?');
        assert!(state.is_error());
    }

    #[rstest]
    fn io_block_reader_block_update_leading_junk(message_1: Record) {
        let bytes = message_1.to_bytes().unwrap();
        let io_len = bytes.len();

        let mut buffer = BytesMut::new();
        buffer.put_bytes(b'x', 12);
        buffer.put_u16(0x4269);
        buffer.put_u32(io_len as u32);
        buffer.put(&bytes[..]);
        let mut bytes = buffer.iter();

        let mut state = IoBlockReader::Init(0x4269);

        while let Some(byte) = bytes.next()
            && !state.is_block()
        {
            state = state.update(*byte);
        }

        let block = state.try_as_block().unwrap();
        let message = Record::from_bytes(&block).unwrap();
        assert_eq!(message, message_1);
    }

    #[rstest]
    fn rkyv_record(message_1: Record) {
        let bytes = message_1.to_bytes().unwrap();
        eprintln!("bytes.len(): {}", bytes.len());
        let message = Record::from_bytes(&bytes).unwrap();
        assert_eq!(message_1, message);
        assert_ne!(addr_of!(message_1), addr_of!(message))
    }

    #[fixture]
    fn message_1() -> Record {
        Record::Message(Message {
            from: Person { id: 1 },
            to: Person { id: 2 },
            subject: String::from("hello world"),
        })
    }

    #[derive(Debug, Archive, Serialize, Deserialize, PartialEq, Eq)]
    enum Record {
        Person(Person),
        Message(Message),
    }

    #[derive(Debug, Archive, Serialize, Deserialize, PartialEq, Eq)]
    struct Person {
        id: u32,
    }

    #[derive(Debug, Archive, Serialize, Deserialize, PartialEq, Eq)]
    struct Message {
        from: Person,
        to: Person,
        subject: String,
    }

    impl Record {
        fn from_bytes(bytes: &Bytes) -> Result<Record, rancor::Error> {
            rkyv::from_bytes(bytes)
        }

        fn to_bytes(&self) -> Result<Bytes, rancor::Error> {
            let b = rkyv::to_bytes(self)?;
            Ok(Bytes::from_iter(b.to_vec()))
        }
    }
}
