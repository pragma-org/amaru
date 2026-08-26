{-# LANGUAGE DeriveGeneric #-}

module Data.Fixture.Point
    ( Point (..)
    , pointConsensusSlotNo
    , pointSlotNo
    ) where

import Relude

import Data.Aeson
    ( FromJSON (parseJSON)
    , genericParseJSON
    )
import Data.Fixture.Common
    ( snakeCaseOptions
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

instance FromJSON Point where
    parseJSON = genericParseJSON snakeCaseOptions

pointSlotNo :: Point -> LedgerSlot.SlotNo
pointSlotNo Point{slot} =
    LedgerSlot.SlotNo slot

pointConsensusSlotNo :: Point -> SlotNo
pointConsensusSlotNo Point{slot} =
    SlotNo slot
