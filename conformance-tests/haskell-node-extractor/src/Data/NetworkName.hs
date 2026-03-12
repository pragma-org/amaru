module Data.NetworkName
    ( NetworkName (..)
    , networkNameToNetwork
    ) where

import Relude
    ()

import Cardano.Ledger.BaseTypes
    ( Network
    )

import qualified Cardano.Ledger.BaseTypes as Ledger

data NetworkName
    = Mainnet
    | Preprod
    | Preview

networkNameToNetwork :: NetworkName -> Network
networkNameToNetwork = \case
    Mainnet ->
        Ledger.Mainnet
    Preprod ->
        Ledger.Testnet
    Preview ->
        Ledger.Testnet
