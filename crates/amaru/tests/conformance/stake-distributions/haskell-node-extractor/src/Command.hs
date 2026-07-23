module Command
    ( runCommandLine
    ) where

import Relude

import qualified Command.ExtractSnapshot as ExtractSnapshot
import qualified Command.StakeDistribution as StakeDistribution
import Options.Applicative
    ( Parser
    , ParserInfo
    , command
    , execParser
    , fullDesc
    , helper
    , hsubparser
    , info
    , progDesc
    )

data Command
    = ExtractSnapshot ExtractSnapshot.Options
    | StakeDistribution StakeDistribution.Options

runCommandLine :: IO ()
runCommandLine = do
    selectedCommand <- execParser commandParserInfo
    result <- runCommand selectedCommand

    case result of
        Left errorMessage -> do
            putTextLn errorMessage
            exitFailure
        Right () ->
            pure ()

runCommand :: Command -> IO (Either Text ())
runCommand = \case
    ExtractSnapshot options ->
        first ExtractSnapshot.renderError <$> runExceptT (ExtractSnapshot.run options)
    StakeDistribution options ->
        first StakeDistribution.renderError <$> runExceptT (StakeDistribution.run options)

commandParserInfo :: ParserInfo Command
commandParserInfo =
    info
        (helper <*> commandParser)
        (fullDesc <> progDesc "Read a ledger snapshot and extract its current NewEpochState")

commandParser :: Parser Command
commandParser =
    hsubparser $
        command
            "extract"
            ( info
                (ExtractSnapshot <$> ExtractSnapshot.optionsParser)
                (progDesc "Read a ledger snapshot from disk")
            )
            <> command
                "stake-distribution"
                ( info
                    (StakeDistribution <$> StakeDistribution.optionsParser)
                    (progDesc "Write a stake distribution snapshot as pretty JSON")
                )
