module Data.NetworkName
    ( NetworkName (..)
    , networkNameToNetwork
    , networkNameToText
    ) where

import Relude
    ( Text
    )

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

networkNameToText :: NetworkName -> Text
networkNameToText = \case
    Mainnet -> "mainnet"
    Preprod -> "preprod"
    Preview -> "preview"
