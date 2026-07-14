module Command.ValidatePhaseOne.Parse
    ( Options (..)
    , optionsParser
    ) where

import Relude

import Options.Applicative
    ( Parser
    , bashCompleter
    , completer
    , help
    , long
    , metavar
    , strOption
    )

data Options = Options
    { fixturePath :: !FilePath
    }

optionsParser :: Parser Options
optionsParser =
    Options
        <$> strOption
            ( long "file"
                <> metavar "PATH"
                <> completer (bashCompleter "file")
                <> help "Path to a phase-one conformance fixture"
            )
