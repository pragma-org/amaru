{-# LANGUAGE DeriveGeneric #-}

module Data.Fixture.Point
    ( Point (..)
    , pointConsensusSlotNo
    , pointSlotNo
    ) where

import Relude

import Data.Aeson
    ( FromJSON
    )
import qualified Cardano.Ledger.Slot as LedgerSlot
import Ouroboros.Consensus.Block
    ( SlotNo (SlotNo)
    )

data Point = Point
    { slot :: !Word64
    , transactionIndex :: !Word64
    }
    deriving (Generic)

instance FromJSON Point

pointSlotNo :: Point -> LedgerSlot.SlotNo
pointSlotNo Point{slot} =
    LedgerSlot.SlotNo slot

pointConsensusSlotNo :: Point -> SlotNo
pointConsensusSlotNo Point{slot} =
    SlotNo slot
