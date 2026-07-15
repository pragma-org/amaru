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
    { testCasePath :: !FilePath
    }

optionsParser :: Parser Options
optionsParser =
    Options
        <$> strOption
            ( long "test-case"
                <> metavar "PATH"
                <> completer (bashCompleter "file")
                <> help "Path to a phase-one conformance fixture"
            )
