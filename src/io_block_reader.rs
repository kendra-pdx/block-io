use bytes::{Buf, BufMut, Bytes, BytesMut};
use rancor::Source;

#[derive(Debug, thiserror::Error)]
#[error("block was read, but not taken")]
pub struct BlockNotTakenError;

#[derive(Debug, thiserror::Error)]
#[error("block did not begin with expected")]
pub struct ExpectedMismatchError;

#[derive(Default)]
pub struct ReadState {
    bytes_read: usize,
    buffer: BytesMut,
}

#[derive(strum::EnumTryAs, strum::EnumIs)]
pub enum IoBlockReader {
    /// initialize the block reader with the expected signature
    Init(u16),
    /// reading 2 bytes to match the signature
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
    Error(rancor::Error),
}

impl ReadState {
    fn push(&mut self, b: u8) {
        self.buffer.put_u8(b);
        self.bytes_read += 1;
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
                if read_state.bytes_read == size_of::<u16>() {
                    let actual = read_state.buffer.get_u16();
                    if actual == expect {
                        Self::ReadingBlockSize(ReadState::default())
                    } else {
                        Self::Error(rancor::Error::new(ExpectedMismatchError))
                    }
                } else {
                    Self::ReadingExpect { expect, read_state }
                }
            }
            Self::ReadingBlockSize(mut read_state) => {
                read_state.push(byte);
                if read_state.bytes_read == size_of::<u32>() {
                    let block_size = read_state.buffer.get_u32();
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
                if read_state.bytes_read as u32 == block_size {
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
            Self::Block(_) => Self::Error(rancor::Error::new(BlockNotTakenError)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::addr_of;

    use bytes::{BufMut, Bytes, BytesMut};
    use rkyv::{Archive, Deserialize, Serialize};
    use rstest::{fixture, rstest};

    use crate::IoBlockReader;

    #[rstest]
    fn io_block_reader(message_1: Record) {
        let bytes = message_1.to_bytes().unwrap();
        let io_len = bytes.len();

        let mut buffer = BytesMut::new();
        buffer.put_u16(0x4269);
        buffer.put_u32(io_len as u32);
        buffer.put(&bytes[..]);
        let mut bytes = buffer.iter();

        assert_eq!(bytes.len(), size_of::<u16>() + size_of::<u32>() + io_len);

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
    fn io_block_reader_block_update_block_not_taken(message_1: Record) {
        let bytes = message_1.to_bytes().unwrap();
        let io_len = bytes.len();

        let mut buffer = BytesMut::new();
        buffer.put_u16(0x4269);
        buffer.put_u32(io_len as u32);
        buffer.put(&bytes[..]);
        let mut bytes = buffer.iter();

        assert_eq!(bytes.len(), size_of::<u16>() + size_of::<u32>() + io_len);

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
    fn io_block_reader_block_update_unexpected(message_1: Record) {
        let bytes = message_1.to_bytes().unwrap();
        let io_len = bytes.len();

        let mut buffer = BytesMut::new();
        buffer.put_u16(0x0000);
        buffer.put_u32(io_len as u32);
        buffer.put(&bytes[..]);
        let mut bytes = buffer.iter();

        assert_eq!(bytes.len(), size_of::<u16>() + size_of::<u32>() + io_len);

        let mut state = IoBlockReader::Init(0x4269);

        while let Some(byte) = bytes.next()
            && !state.is_block()
        {
            state = state.update(*byte);
        }

        assert!(state.is_error());

        state = state.update(b'?');
        assert!(state.is_error());
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
