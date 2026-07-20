{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE DuplicateRecordFields #-}

module Data.Fixture.EraHistory
    ( EraBound (..)
    , EraHistory (..)
    , EraParameters (..)
    , FixtureEraSummary (..)
    , buildEpochInfo
    , pointEpochNo
    ) where

import Relude

import Command.ValidatePhaseOne.Error
    ( Error (..)
    )
import Data.Aeson
    ( FromJSON
    )
import Data.Fixture.Common
    ( perasDisabled
    , showText
    )
import Data.Fixture.Point
    ( Point
    , pointConsensusSlotNo
    )
import qualified Cardano.Ledger.Slot as LedgerSlot
import qualified Cardano.Slotting.EpochInfo as EpochInfo
import Cardano.Slotting.EpochInfo
    ( hoistEpochInfo
    )
import Cardano.Slotting.Time
    ( RelativeTime (RelativeTime)
    )
import Ouroboros.Consensus.Block
    ( EpochNo (EpochNo)
    , EpochSize (EpochSize)
    , SlotNo (SlotNo)
    )
import Ouroboros.Consensus.Block.Abstract
    ( GenesisWindow (GenesisWindow)
    )
import Ouroboros.Consensus.BlockchainTime.WallClock.Types
    ( slotLengthFromMillisec
    )
import Ouroboros.Consensus.HardFork.History.EpochInfo
    ( summaryToEpochInfo
    )
import Ouroboros.Consensus.HardFork.History.EraParams
    ( EraParams (..)
    , SafeZone (UnsafeIndefiniteSafeZone)
    )
import Ouroboros.Consensus.HardFork.History.Summary
    ( Bound (..)
    , EraEnd (EraEnd)
    , Summary (..)
    , mkUpperBound
    )
import qualified Ouroboros.Consensus.HardFork.History.Summary as HardForkHistory
import qualified Data.SOP.NonEmpty as SOPNonEmpty

data EraHistory = EraHistory
    { stabilityWindow :: !Word64
    , eras :: ![FixtureEraSummary]
    }
    deriving (Generic)

instance FromJSON EraHistory

data FixtureEraSummary = FixtureEraSummary
    { start :: !EraBound
    , end :: !(Maybe EraBound)
    , params :: !EraParameters
    }
    deriving (Generic)

instance FromJSON FixtureEraSummary

data EraBound = EraBound
    { time :: !Word64
    , slot :: !Word64
    , epoch :: !Word64
    }
    deriving (Generic)

instance FromJSON EraBound

data EraParameters = EraParameters
    { epochSizeSlots :: !Word64
    , slotLengthMs :: !Word64
    , eraName :: !Text
    }
    deriving (Generic)

instance FromJSON EraParameters

buildEpochInfo :: EraHistory -> Point -> Either Error (EpochInfo.EpochInfo (Either Text))
buildEpochInfo eraHistory point = do
    fixtureEraSummary <- singleEraSummary eraHistory
    startBound <- buildBound (start fixtureEraSummary)
    eraParams <- buildEraParams (params fixtureEraSummary)
    endBound <- case end fixtureEraSummary of
        Just boundedEnd ->
            EraEnd <$> buildBound boundedEnd
        Nothing -> do
            let SlotNo startSlot = boundSlot startBound
            let EpochNo startEpoch = boundEpoch startBound
            let SlotNo currentSlot = pointConsensusSlotNo point
            let EpochSize epochSize = eraEpochSize eraParams
            when (currentSlot < startSlot) $
                Left (UnsupportedFixture "the fixture point is before the era-history start bound")
            let currentEpoch = startEpoch + ((currentSlot - startSlot) `div` epochSize)
            pure (EraEnd (mkUpperBound eraParams startBound (EpochNo (currentEpoch + 1))))

    let summary :: Summary '[()]
        summary =
            Summary
                ( SOPNonEmpty.NonEmptyOne
                    HardForkHistory.EraSummary
                        { eraStart = startBound
                        , eraEnd = endBound
                        , eraParams = eraParams
                        }
                )

    pure (hoistEpochInfo (first showText . runIdentity . runExceptT) (summaryToEpochInfo summary))

pointEpochNo :: EraHistory -> Point -> Either Error LedgerSlot.EpochNo
pointEpochNo eraHistory point = do
    FixtureEraSummary
        { start = EraBound{slot = eraStartSlot, epoch = eraStartEpoch}
        , params = EraParameters{epochSizeSlots}
        } <- singleEraSummary eraHistory
    let SlotNo currentSlot = pointConsensusSlotNo point
    when (currentSlot < eraStartSlot) $
        Left (UnsupportedFixture "the fixture point is before the era-history start bound")
    pure (LedgerSlot.EpochNo (eraStartEpoch + ((currentSlot - eraStartSlot) `div` epochSizeSlots)))

buildBound :: EraBound -> Either Error Bound
buildBound EraBound{time, slot, epoch} =
    pure
        Bound
            { boundTime = RelativeTime (fromIntegral time)
            , boundSlot = SlotNo slot
            , boundEpoch = EpochNo epoch
            , boundPerasRound = perasDisabled
            }

buildEraParams :: EraParameters -> Either Error EraParams
buildEraParams EraParameters{epochSizeSlots, slotLengthMs, eraName}
    | eraName /= "Conway" =
        Left (UnsupportedFixture ("unsupported era name in era history: " <> eraName))
    | otherwise =
        pure
            EraParams
                { eraEpochSize = EpochSize epochSizeSlots
                , eraSlotLength = slotLengthFromMillisec (fromIntegral slotLengthMs)
                , eraSafeZone = UnsafeIndefiniteSafeZone
                , eraGenesisWin = GenesisWindow 0
                , eraPerasRoundLength = perasDisabled
                }

singleEraSummary :: EraHistory -> Either Error FixtureEraSummary
singleEraSummary EraHistory{eras = [singleEra]} =
    Right singleEra
singleEraSummary EraHistory{} =
    Left (UnsupportedFixture "only single-era Conway phase-one fixtures are supported")
