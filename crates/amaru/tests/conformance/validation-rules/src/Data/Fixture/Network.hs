module Data.Fixture.Network
    ( FixtureNetwork (..)
    , fixtureNetworkId
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( Network (Mainnet, Testnet)
    )
import Data.Aeson
    ( FromJSON (parseJSON)
    , withText
    )
import qualified Data.Text as Text

newtype FixtureNetwork = FixtureNetwork
    { unFixtureNetwork :: Text
    }

instance FromJSON FixtureNetwork where
    parseJSON =
        withText "FixtureNetwork" $ \networkName ->
            if networkName == "mainnet"
                || networkName == "preprod"
                || networkName == "preview"
                || "testnet_" `Text.isPrefixOf` networkName
                then pure (FixtureNetwork networkName)
                else fail ("Unsupported network: " <> toString networkName)

fixtureNetworkId :: FixtureNetwork -> Network
fixtureNetworkId (FixtureNetwork networkName)
    | networkName == "mainnet" =
        Mainnet
    | otherwise =
        Testnet
