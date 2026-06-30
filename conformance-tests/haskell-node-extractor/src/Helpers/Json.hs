module Helpers.Json
    ( snakeCaseFieldLabel
    , snakeCaseOptions
    , writeJsonOutput
    ) where

import Relude

import Data.Aeson
    ( Options (fieldLabelModifier)
    , ToJSON
    , camelTo2
    , defaultOptions
    )
import Data.Aeson.Encode.Pretty
    ( encodePretty
    )
import System.Directory
    ( createDirectoryIfMissing
    )
import System.FilePath
    ( takeDirectory
    )

snakeCaseFieldLabel :: String -> String
snakeCaseFieldLabel =
    camelTo2 '_'

snakeCaseOptions :: Options
snakeCaseOptions =
    defaultOptions
        { fieldLabelModifier = snakeCaseFieldLabel
        }

writeJsonOutput :: ToJSON a => FilePath -> a -> IO ()
writeJsonOutput outputPath jsonValue = do
    putTextLn ("...extracting " <> toText outputPath)
    createDirectoryIfMissing True (takeDirectory outputPath)
    writeFileLBS outputPath (encodePretty jsonValue)
