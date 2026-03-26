{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.PoolDelegator
    ( PoolDelegator (..)
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.Keys
    ( KeyRole (Staking)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.Credential
    ( credentialPairs
    )

data PoolDelegator = PoolDelegator
    { credential :: !(Credential 'Staking)
    , stake :: !Coin
    }

instance ToJSON PoolDelegator where
    toJSON PoolDelegator{credential, stake} =
        object
            ( credentialPairs credential
                <> [ "stake" .= JsonCoin stake
                   ]
            )
