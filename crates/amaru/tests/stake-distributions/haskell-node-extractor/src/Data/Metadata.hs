{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE MagicHash #-}

module Data.Metadata
    ( JsonContentHash (..)
    , JsonUrl (..)
    , Metadata (..)
    , metadataFromAnchor
    , metadataFromPoolMetadata
    ) where

import Relude

import Data.Array.Byte
    ( ByteArray (ByteArray)
    )
import Cardano.Ledger.BaseTypes
    ( Anchor
        ( Anchor
        , anchorDataHash
        , anchorUrl
        )
    , urlToText
    )
import Cardano.Ledger.State
    ( PoolMetadata
        ( pmHash
        , pmUrl
        )
    )
import Data.Aeson
    ( KeyValue ((.=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object, String)
    , pairs
    )
import Data.HexString
    ( bytesToHexText
    )
import GHC.Exts
    ( Int (I#)
    , (+#)
    , (>=#)
    , indexWord8Array#
    , isTrue#
    , sizeofByteArray#
    , word8ToWord#
    , word2Int#
    )

import qualified Data.ByteString as BS

newtype JsonUrl = JsonUrl
    { unJsonUrl :: Text
    }

instance ToJSON JsonUrl where
    toJSON =
        toJSON . unJsonUrl

newtype JsonContentHash = JsonContentHash
    { unJsonContentHash :: Text
    }

instance ToJSON JsonContentHash where
    toJSON =
        toJSON . unJsonContentHash

data Metadata = Metadata
    { url :: !JsonUrl
    , contentHash :: !JsonContentHash
    }
    deriving (Generic)

instance ToJSON Metadata where
    toJSON =
        Object . metadataFields

    toEncoding =
        pairs . metadataFields

metadataFields :: (KeyValue e kv, Monoid kv) => Metadata -> kv
metadataFields Metadata{url, contentHash} = mempty
    <> "url" .= url
    <> "content_hash" .= contentHash

metadataFromPoolMetadata :: PoolMetadata -> Metadata
metadataFromPoolMetadata poolMetadata =
    Metadata
        { url = JsonUrl (urlToText (pmUrl poolMetadata))
        , contentHash = JsonContentHash (bytesToHexText (byteArrayToByteString (pmHash poolMetadata)))
        }

metadataFromAnchor :: Anchor -> Metadata
metadataFromAnchor Anchor{anchorDataHash, anchorUrl} =
    Metadata
        { url = JsonUrl (urlToText anchorUrl)
        , contentHash = JsonContentHash (safeHashToText anchorDataHash)
        }

safeHashToText :: ToJSON hash => hash -> Text
safeHashToText hashValue =
    case toJSON hashValue of
        String text ->
            text
        _ ->
            error "SafeHash ToJSON did not produce a JSON string"

byteArrayToByteString :: ByteArray -> ByteString
byteArrayToByteString (ByteArray byteArray#) =
    BS.pack (go 0#)
  where
    size# =
        sizeofByteArray# byteArray#

    go index#
        | isTrue# (index# >=# size#) =
            []
        | otherwise =
            fromIntegral (I# (word2Int# (word8ToWord# (indexWord8Array# byteArray# index#)))) : go (index# +# 1#)
