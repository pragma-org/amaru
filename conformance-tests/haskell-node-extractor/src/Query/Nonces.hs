module Query.Nonces
    ( Nonces (..)
    , noncesOutputPath
    , queryNonces
    ) where

import Relude

import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Nonce
    ( JsonNonce (JsonNonce)
    )
import Ouroboros.Consensus.Protocol.Praos
    ( PraosState
    , praosStateCandidateNonce
    , praosStateEpochNonce
    , praosStateEvolvingNonce
    , praosStateLastEpochBlockNonce
    )

newtype Nonces = Nonces
    { unNonces :: PraosState
    }

instance ToJSON Nonces where
    toJSON (Nonces praosState) =
        object
            [ "epochNonce" .= JsonNonce (praosStateEpochNonce praosState)
            , "candidateNonce" .= JsonNonce (praosStateCandidateNonce praosState)
            , "evolvingNonce" .= JsonNonce (praosStateEvolvingNonce praosState)
            , "lastEpochLastAncestor" .= JsonNonce (praosStateLastEpochBlockNonce praosState)
            ]

queryNonces :: PraosState -> Nonces
queryNonces =
    Nonces

noncesOutputPath :: Word64 -> FilePath
noncesOutputPath epochNumber =
    "data/nonces/" <> toString (show epochNumber :: Text) <> ".json"
