// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::echo::Envelope;
use crate::sync::ChainSyncMessage;
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin, stdin};
use tracing::error;

/// This trait supports reading and parsing ChainSyncMessage messages from an input source.
/// There are 2 implementations provided:
/// - StdinMessageReader: reads messages from standard input (stdin).
/// - StringMessageReader: reads messages from a vector of strings (for testing).
#[async_trait]
pub trait MessageReader: Send {
    async fn read(&mut self) -> Result<Envelope<ChainSyncMessage>, ReaderError>;
}

#[derive(Debug)]
pub enum InitError {
    IOError(ReaderError),
    DecodingError(String),
    NotInitMessage(ChainSyncMessage),
}

/// Read and parse an Init message from the provided MessageReader.
/// Return the list of peer addresses contained in the Init message.
pub async fn read_peer_addresses_from_init(
    reader: &mut impl MessageReader,
) -> Result<Vec<String>, InitError> {
    let input = reader.read().await.map_err(InitError::IOError)?;
    match input.body {
        ChainSyncMessage::Init { node_ids, .. } => Ok(node_ids),
        msg => Err(InitError::NotInitMessage(msg)),
    }
}

#[derive(Debug, PartialEq)]
pub enum ReaderError {
    IOError(String),
    JSONError(String),
    EndOfFile,
}

/// Implementation of a MessageReader off the standard input.
pub struct StdinMessageReader {
    reader: Lines<BufReader<Stdin>>,
}

impl Default for StdinMessageReader {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinMessageReader {
    pub fn new() -> Self {
        let reader = BufReader::new(stdin()).lines();
        Self { reader }
    }
}

#[async_trait]
impl MessageReader for StdinMessageReader {
    async fn read(&mut self) -> Result<Envelope<ChainSyncMessage>, ReaderError> {
        let next_input = Lines::next_line(&mut self.reader).await.map_err(|err| {
            error!("failed to read line from stdin: {}", err);
            ReaderError::IOError(err.to_string())
        })?;

        parse(&next_input)
    }
}

/// Implementation of a MessageReader off a vector of strings for testing purposes.
pub struct StringMessageReader {
    lines: Vec<String>,
    index: usize,
}

impl From<Vec<String>> for StringMessageReader {
    fn from(lines: Vec<String>) -> Self {
        Self { lines, index: 0 }
    }
}

#[async_trait]
impl MessageReader for StringMessageReader {
    async fn read(&mut self) -> Result<Envelope<ChainSyncMessage>, ReaderError> {
        if self.index >= self.lines.len() {
            return Err(ReaderError::EndOfFile);
        }

        let msg = parse(&Some(self.lines[self.index].clone()));
        self.index += 1;
        msg
    }
}

/// Parse a ChainSyncMessage from a JSON string provided as an input line.
/// Return an EndOfFile if the input line is None.
fn parse(next_input: &Option<String>) -> Result<Envelope<ChainSyncMessage>, ReaderError> {
    use ReaderError::*;

    match next_input {
        Some(line) => match serde_json::from_str::<Envelope<ChainSyncMessage>>(line) {
            Ok(v) => Ok(v),
            Err(err) => Err(JSONError(err.to_string())),
        },
        None => Err(EndOfFile), // EOF
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn can_read_init_message_with_some_peer_addresses() {
        let init_string = r#"{"body":{"node_id":"c0","node_ids":["n1","n2"],"type":"init","msg_id":0},"dest":"c0","src":"c0"}"#;
        let mut input = StringMessageReader::from(vec![init_string.to_string()]);
        let envelope = read_peer_addresses_from_init(&mut input).await;

        assert_eq!(envelope.unwrap(), vec!["n1".to_string(), "n2".to_string()]);
    }

    #[tokio::test]
    async fn returns_error_when_reading_addresses_given_message_is_not_init() {
        let mut input = StringMessageReader::from(vec![TEST_FWD_MSG.to_string()]);
        let envelope = read_peer_addresses_from_init(&mut input).await;

        assert!(envelope.is_err());
    }

    #[test]
    fn returns_error_when_parsing_message_fails() {
        assert_eq!(
            parse(&Some("foo".to_string())),
            Err(ReaderError::JSONError(
                "expected ident at line 1 column 2".to_string()
            ))
        );
    }

    // HELPERS

    const TEST_FWD_MSG: &str = r#"{"src":"n2","dest":"n1","body":{"type":"fwd","msg_id":1,"hash":"2487bd4f49c89e59bb3d2166510d3d49017674d3c3b430b95db2e260fedce45e","header":"828a01181ff6582022ff37595005fa65ad731d4fb112de050aa0ca910d9a3110f56f3879d449f88e5820da90997ba81483b3f2b4de751ba3ece6ba6d50f96598eb0940cc7d29452cdcb9825840d350e19abe11a25b28d4d1a846faa8f7792b6dc4a19679780dcdd3d98baf4e9738d82764c59b1c76f80b5ddbff2e145aa26f1652ab83f1f930fc430d1be960305850b444cc452b3fbc01a267ff526ea8a66cb57202303b4b1c22557cad8c1d8afbb47a34e55c5e0bee497f58c812e8b2fed95b40cb1f966ad77445ef573f289910debf5dcb4924cecce47d181f325b4d21040058200a9aa01fbdfe2cff3e49a3c02c2610691966075092f76bd26f5bddf85489ffd3845820499fc5dada1544be26d7cd3b2851fe955b44e560b50901abc71333d5d449eac9182e005840595a9a329b2637b8f2cb501aa793a159acc928a27c01e1ef586508492a68cabeae6ced714421be1f648bb05c7196f38e7aa4a8f616ad46e32c84e67951657a038200005901c0c00b0daf83dfc61a23fa9f6e4db5ad31428a7e98839aba420dbdbdc5ad90b72185cb03b5373b73fcd9c0128b3afcc7d14e5b51f4d5592d55a5d222314b590101472338fcc8b178f0ba10f8683725c5df444b7fc6a5afc3c7fbab82e00e7df2d247c673d066eacc1dc7860c10b134c413d71c5a073a5f3e66a17f0d25dabae2699d7b4a969e129d627cd7839995ddf40a3e6672b6d03936de782f5e0dc31bfafdccc5d5d2e5e9a5b3414bface59a824e3a574250474d633115af821b63232de753fddb638606b93b853144dd75692b02e73b2ef8621eb1bfa307cfda8acaa2c43f8b673c9ed749e472cdf2fced20c063a2507ccb985d2b5a9bc77699f42379fb349e1e3a1ab86d1bd510c6ee89720f860d55c208dd262f49746d8fb2a7817d038262a6b266f98f427fc8b958f11adb8c84cc96444b1f5f9d994a4fd1adae2f2be87d8c1dbc0206ace7871da50cc92476d3fced6a4fc2809c8bb47dff992b06259c21f78cd6e72b49b8fe101065aaf243003af67a11d5aabcebf1059b3c92493f954507573a01bf92047ef68f4e980df914b360307b78b3371138b4c1504e155f704df9fabf1bc87ac47b3d5cd563cee6017380e4df6529face9cf39b36277068e","height":1,"parent":null,"slot":31}}"#;
}
