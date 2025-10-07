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

use amaru_kernel::{Header, Point, RawBlock, peer::Peer};
use amaru_ouroboros_traits::IsHeader;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Debug, Formatter};
use tracing::Span;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChainSyncEvent<T> {
    RollForward {
        peer: Peer,
        point: Point,
        value: T,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    CaughtUp {
        peer: Peer,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl<T> ChainSyncEvent<T> {
    pub fn peer(&self) -> &Peer {
        match self {
            ChainSyncEvent::RollForward { peer, .. } => peer,
            ChainSyncEvent::Rollback { peer, .. } => peer,
            ChainSyncEvent::CaughtUp { peer, .. } => peer,
        }
    }
}

impl<T: Debug> Debug for ChainSyncEvent<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ChainSyncEvent::RollForward {
                peer, point, value, ..
            } => f
                .debug_struct("RollForward")
                .field("peer", &peer.name)
                .field("point", &point.to_string())
                .field("value", value)
                .finish(),
            ChainSyncEvent::Rollback { peer, point, .. } => f
                .debug_struct("Rollback")
                .field("peer", &peer.name)
                .field("point", &point.to_string())
                .finish(),
            ChainSyncEvent::CaughtUp { peer, .. } => f
                .debug_struct("CaughtUp")
                .field("peer", &peer.name)
                .finish(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NewHeader {
    peer: Peer,
    point: Point,
    header: Header,
    #[serde(skip, default = "Span::none")]
    span: Span,
}

impl NewHeader {
    pub fn new(peer: Peer, point: Point, header: Header) -> Self {
        Self {
            peer,
            point,
            header,
            span: Span::current(),
        }
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn point(&self) -> Point {
        self.header.point()
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

impl Debug for NewHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewHeader")
            .field("peer", &self.peer)
            .field("header", &self.header)
            .finish()
    }
}

impl PartialEq for NewHeader {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header && self.peer == other.peer
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ForwardHeader {
    RollForward {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl Debug for ForwardHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ForwardHeader::RollForward { peer, header, .. } => f
                .debug_struct("ForwardHeaderRollForward")
                .field("peer", &peer.name)
                .field("header", &header.hash())
                .finish(),
            ForwardHeader::Rollback { peer, header, .. } => f
                .debug_struct("ForwardHeader:Rollback")
                .field("peer", &peer.name)
                .field("header", &header.hash())
                .finish(),
        }
    }
}

impl PartialEq for ForwardHeader {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ForwardHeader::RollForward {
                    peer: p1,
                    header: h1,
                    ..
                },
                ForwardHeader::RollForward {
                    peer: p2,
                    header: h2,
                    ..
                },
            ) => p1 == p2 && h1 == h2,
            (
                ForwardHeader::Rollback {
                    peer: p1,
                    header: h1,
                    ..
                },
                ForwardHeader::Rollback {
                    peer: p2,
                    header: h2,
                    ..
                },
            ) => p1 == p2 && h1 == h2,
            _ => false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FetchBlock {
    peer: Peer,
    header: Header,
    #[serde(skip, default = "Span::none")]
    span: Span,
}

impl FetchBlock {
    pub fn new(peer: Peer, header: Header) -> Self {
        Self {
            peer,
            header,
            span: Span::current(),
        }
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

impl Debug for FetchBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchBlock")
            .field("peer", &self.peer)
            .field("header", &self.header)
            .finish()
    }
}

impl PartialEq for FetchBlock {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header && self.peer == other.peer
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoreBlock {
    peer: Peer,
    header: Header,
    #[serde(skip, default = "RawBlock::default")]
    block: RawBlock,
    #[serde(skip, default = "Span::none")]
    span: Span,
}

impl StoreBlock {
    pub fn new(peer: Peer, header: Header, block: RawBlock, span: Span) -> Self {
        Self {
            peer,
            header,
            block,
            span,
        }
    }

    pub fn from_fetch(msg: FetchBlock, block: RawBlock) -> Self {
        Self {
            peer: msg.peer,
            header: msg.header,
            block,
            span: Span::current(),
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn block(&self) -> &RawBlock {
        &self.block
    }

    pub fn into_block(self) -> RawBlock {
        self.block
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

impl Debug for StoreBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreBlock")
            .field("peer", &self.peer)
            .field("header", &self.header)
            .field("block", &self.block)
            .finish()
    }
}

impl PartialEq for StoreBlock {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header && self.peer == other.peer
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ValidateBlock {
    RollForward {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "RawBlock::default")]
        block: RawBlock,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl ValidateBlock {
    pub fn new_roll_forward(peer: Peer, header: Header, block: RawBlock) -> Self {
        Self::RollForward {
            peer,
            header,
            block,
            span: Span::current(),
        }
    }
}

impl Debug for ValidateBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ValidateBlock::RollForward { peer, header, .. } => f
                .debug_struct("LedgerBlockEvent:RollForward")
                .field("peer", &peer.name)
                .field("header", &header.hash())
                .finish(),
            ValidateBlock::Rollback { peer, point, .. } => f
                .debug_struct("LedgerBlockEvent:Rollback")
                .field("peer", &peer.name)
                .field("point", &point.hash())
                .finish(),
        }
    }
}

impl PartialEq for ValidateBlock {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ValidateBlock::RollForward {
                    peer: p1,
                    header: h1,
                    ..
                },
                ValidateBlock::RollForward {
                    peer: p2,
                    header: h2,
                    ..
                },
            ) => p1 == p2 && h1 == h2,
            (
                ValidateBlock::Rollback {
                    peer: peer1,
                    point: point1,
                    ..
                },
                ValidateBlock::Rollback {
                    peer: peer2,
                    point: point2,
                    ..
                },
            ) => peer1 == peer2 && point1 == point2,
            _ => false,
        }
    }
}
