{-# LANGUAGE TypeApplications #-}

module Command.ValidateMaths
    ( Options (..)
    , optionsParser
    , run
    ) where

import Relude

import Cardano.Ledger.NonIntegral
    ( CompareResult (..)
    , ln'
    , taylorExpCmp
    )
import Data.Aeson
    ( Value
    , encode
    , object
    , (.=)
    )
import Data.Fixed
    ( Fixed (MkFixed)
    , HasResolution (resolution)
    )
import Data.Time.Clock
    ( UTCTime
    , getCurrentTime
    )
import Options.Applicative
    ( Parser
    , bashCompleter
    , completer
    , help
    , long
    , metavar
    , strOption
    , value
    )

import qualified Data.ByteString.Lazy as LazyByteString
import qualified Data.Text as Text
import qualified Data.Text.IO as Text.IO

data Options = Options
    { primitives :: !FilePath
    , comparisons :: !FilePath
    , leaderThresholds :: !FilePath
    , jsonReport :: !(Maybe FilePath)
    }
  deriving (Eq, Show)

optionsParser :: Parser Options
optionsParser =
    Options
        <$> pathOption
            "primitives"
            "PATH"
            "TSV rows for fixed-point primitive operations"
            "../../../../amaru-kernel/tests/data/maths/node-primitives.tsv"
        <*> pathOption
            "comparisons"
            "PATH"
            "TSV rows for exponential comparisons"
            "../../../../amaru-kernel/tests/data/maths/node-comparisons.tsv"
        <*> pathOption
            "leader-thresholds"
            "PATH"
            "TSV rows for VRF leader-threshold checks"
            "../../../../amaru-ouroboros/tests/data/leader-thresholds.tsv"
        <*> optional
            ( strOption
                ( long "json-report"
                    <> metavar "PATH"
                    <> completer (bashCompleter "file")
                    <> help "Write the JSON report to PATH instead of stdout"
                )
            )
  where
    pathOption name argument description defaultPath =
        strOption
            ( long name
                <> metavar argument
                <> value defaultPath
                <> completer (bashCompleter "file")
                <> help description
            )

data E34

instance HasResolution E34 where
    resolution _ = 10 ^ (34 :: Int)

type FixedDecimal = Fixed E34

raw :: Integer -> FixedDecimal
raw = MkFixed

unRaw :: FixedDecimal -> Integer
unRaw (MkFixed fixedValue) = fixedValue

whole :: Integer -> FixedDecimal
whole = fromInteger

data Result = Result
    { resultValue :: !Value
    }

run :: Options -> ExceptT Text IO ()
run Options{primitives, comparisons, leaderThresholds, jsonReport} = do
    primitiveRows <- readRows primitives
    comparisonRows <- readRows comparisons
    leaderRows <- readRows leaderThresholds

    primitiveResults <- traverse (hoistEither . uncurry computePrimitive) (zip [1 ..] primitiveRows)
    comparisonResults <- traverse (hoistEither . uncurry computeComparison) (zip [1 ..] comparisonRows)
    leaderResults <- traverse (hoistEither . uncurry computeLeaderThreshold) (zip [1 ..] leaderRows)

    generatedAt <- liftIO getCurrentTime
    let reportValue = report generatedAt (map resultValue (primitiveResults <> comparisonResults <> leaderResults))
    case jsonReport of
        Just path -> liftIO (LazyByteString.writeFile path (encode reportValue))
        Nothing -> liftIO (putLBSLn (encode reportValue))

readRows :: FilePath -> ExceptT Text IO [[Text]]
readRows path = do
    contents <- liftIO (Text.IO.readFile path)
    pure
        [ Text.splitOn "\t" line
        | line <- Text.lines contents
        , not (Text.null line)
        , not ("#" `Text.isPrefixOf` line)
        ]

computePrimitive :: Int -> [Text] -> Either Text Result
computePrimitive index columns = do
    [operation, a, b, _expected] <- expectColumns "primitive" index 4 columns
    lhs <- parseInteger "primitive" index a
    rhs <- parseInteger "primitive" index b
    actual <- case operation of
        "multiply" -> pure (raw lhs * raw rhs)
        "divide" -> pure (raw lhs / raw rhs)
        "ratio" -> pure (whole lhs / whole rhs)
        _ -> Left (rowError "primitive" index ("unknown operation: " <> operation))
    let actualValue = unRaw actual
        result = object
            [ "id" .= rowId "node-primitives" index
            , "suite" .= ("node-primitives" :: Text)
            , "result" .= object
                [ "operation" .= operation
                , "value" .= showInteger actualValue
                ]
            ]
    pure Result{resultValue = result}

computeComparison :: Int -> [Text] -> Either Text Result
computeComparison index columns = do
    [compareValue, x, _expectedOrdering, _expectedIterations] <- expectColumns "comparison" index 4 columns
    compareInteger <- parseInteger "comparison" index compareValue
    xInteger <- parseInteger "comparison" index x
    let actual = taylorExpCmp 3 (raw compareInteger) (raw xInteger)
        (actualOrdering, actualIterations) = compareResult actual
        result = object
            [ "id" .= rowId "node-comparisons" index
            , "suite" .= ("node-comparisons" :: Text)
            , "result" .= object
                [ "ordering" .= actualOrdering
                , "iterations" .= showInteger actualIterations
                ]
            ]
    pure Result{resultValue = result}

computeLeaderThreshold :: Int -> [Text] -> Either Text Result
computeLeaderThreshold index columns = do
    [rawNumerator, rawDenominator, rawNatural, _expectedEligible, _expectedSigma, _expectedLog, _expectedReciprocal, _expectedExponent, _expectedOrdering, _expectedIterations] <-
        expectColumns "leader-threshold" index 10 columns
    numeratorValue <- parseInteger "leader-threshold" index rawNumerator
    denominatorValue <- parseInteger "leader-threshold" index rawDenominator
    naturalValue <- parseInteger "leader-threshold" index rawNatural

    let coefficient = whole 1 / whole 20
        sigma = whole numeratorValue / whole denominatorValue
        logValue = ln' (1 - coefficient)
        certifiedNaturalMax = whole (2 ^ (256 :: Int))
        reciprocal = certifiedNaturalMax / (certifiedNaturalMax - whole naturalValue)
        exponent = negate (sigma * logValue)
        comparison = taylorExpCmp 3 reciprocal exponent
        (actualOrdering, actualIterations) = compareResult comparison
        actualEligible = actualOrdering == "BELOW"
        result = object
            [ "id" .= rowId "leader-thresholds" index
            , "suite" .= ("leader-thresholds" :: Text)
            , "result" .= object
                [ "eligible" .= actualEligible
                , "sigma" .= showInteger (unRaw sigma)
                , "log" .= showInteger (unRaw logValue)
                , "reciprocal" .= showInteger (unRaw reciprocal)
                , "exponent" .= showInteger (unRaw exponent)
                , "ordering" .= actualOrdering
                , "iterations" .= showInteger actualIterations
                ]
            ]
    pure Result{resultValue = result}

compareResult :: CompareResult FixedDecimal -> (Text, Integer)
compareResult = \case
    BELOW _ iterations -> ("BELOW", fromIntegral iterations)
    ABOVE _ iterations -> ("ABOVE", fromIntegral iterations)
    MaxReached iterations -> ("MAX_REACHED", fromIntegral iterations)

expectColumns :: Text -> Int -> Int -> [Text] -> Either Text [Text]
expectColumns suite index expected columns
    | length columns == expected = Right columns
    | otherwise = Left (rowError suite index ("expected " <> show expected <> " columns, got " <> show (length columns)))

parseInteger :: Text -> Int -> Text -> Either Text Integer
parseInteger suite index rawText =
    maybe (Left (rowError suite index ("invalid integer: " <> rawText))) Right (readMaybe (toString rawText))

rowError :: Text -> Int -> Text -> Text
rowError suite index message = suite <> " row " <> show index <> ": " <> message

rowId :: Text -> Int -> Text
rowId suite index = suite <> "/" <> show index

showInteger :: Integer -> Text
showInteger = show

report :: UTCTime -> [Value] -> Value
report generatedAt results =
    object
        [ "version" .= ("1" :: Text)
        , "scenario" .= ("ouroboros-maths" :: Text)
        , "generated_at" .= generatedAt
        , "implementation" .= object ["name" .= ("cardano-node" :: Text)]
        , "results" .= results
        ]
