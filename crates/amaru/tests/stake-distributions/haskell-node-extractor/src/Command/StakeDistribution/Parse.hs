module Command.StakeDistribution.Parse
    ( Options (..)
    , optionsParser
    ) where

import Relude

import Command.ExtractSnapshot.Parse
    ( networkNameParser
    )
import Data.NetworkName
    ( NetworkName
    )
import Options.Applicative
    ( Parser
    , bashCompleter
    , completer
    , help
    , long
    , metavar
    , showDefault
    , strOption
    , value
    )

data Options = Options
    { networkName :: !NetworkName
    , snapshotPath :: !FilePath
    , nextSnapshotPath :: !FilePath
    , outputDir :: !FilePath
    }

optionsParser :: Parser Options
optionsParser =
    Options
        <$> networkNameParser
        <*> strOption
            ( long "snapshot"
                <> metavar "PATH"
                <> completer (bashCompleter "directory")
                <> help "Path to the consensus ledger snapshot directory for the target epoch"
            )
        <*> strOption
            ( long "next-snapshot"
                <> metavar "PATH"
                <> completer (bashCompleter "directory")
                <> help "Path to the consensus ledger snapshot directory for the following epoch"
            )
        <*> strOption
            ( long "output"
                <> metavar "DIR"
                <> completer (bashCompleter "directory")
                <> value ".."
                <> showDefault
                <> help "Base directory where the extracted JSON file will be written"
            )
