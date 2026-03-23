import { appendFileSync } from 'node:fs';

function roundTo(value, decimals = 6) {
  const factor = 10 ** decimals;
  return Math.round(value * factor) / factor;
}

function safeNumber(value, fallback = 0) {
  return Number.isFinite(value) ? value : fallback;
}

function formatTraceDescriptor(descriptor) {
  const bits = [`target=${descriptor.target}`];
  if (descriptor.spanName) bits.push(`span=${descriptor.spanName}`);
  return bits.join(' | ');
}

function serializeFailure(failure) {
  if (failure.kind === 'missing-trace-descriptor') {
    return {
      kind: failure.kind,
      traceDescriptor: formatTraceDescriptor(failure.traceDescriptor),
    };
  }

  if (failure.kind === 'sequence-order-changed') {
    return {
      kind: failure.kind,
      before: formatTraceDescriptor(failure.before),
      after: formatTraceDescriptor(failure.after),
    };
  }

  if (failure.kind === 'metric-regression' || failure.kind === 'outlier-regression') {
    return failure;
  }

  return failure;
}

function colorize(text, code, enabled) {
  if (!enabled) return text;
  return `\u001b[${code}m${text}\u001b[0m`;
}

function bold(text, enabled) {
  return colorize(text, '1', enabled);
}

function dim(text, enabled) {
  return colorize(text, '2', enabled);
}

function green(text, enabled) {
  return colorize(text, '32', enabled);
}

function red(text, enabled) {
  return colorize(text, '31', enabled);
}

function cyan(text, enabled) {
  return colorize(text, '36', enabled);
}

function truncateText(value, maxLength) {
  const text = String(value ?? '');
  if (text.length <= maxLength) return text;
  if (maxLength <= 3) return text.slice(0, maxLength);
  return `${text.slice(0, maxLength - 3)}...`;
}

function padText(value, width) {
  return truncateText(value, width).padEnd(width, ' ');
}

function formatMs(value) {
  return Number.isFinite(value) ? `${roundTo(value, 3)} ms` : 'n/a';
}

function formatPercent(value) {
  return Number.isFinite(value) ? `${roundTo(value * 100, 1)}%` : 'n/a';
}

function supportsUnicode(stream) {
  if (!stream?.isTTY) {
    return false;
  }

  const locale = `${process.env.LC_ALL ?? ''}${process.env.LC_CTYPE ?? ''}${process.env.LANG ?? ''}`.toLowerCase();
  return locale.includes('utf-8') || locale.includes('utf8');
}

function formatDescriptorLabel(descriptor, maxLength = 56) {
  const label = descriptor.spanName
    ? `${descriptor.target}:${descriptor.spanName}`
    : descriptor.target;
  return truncateText(label, maxLength);
}

function formatShareBar(share, width = 14, useUnicode = false) {
  const full = useUnicode ? '█' : '#';
  const empty = useUnicode ? '░' : '-';

  if (!Number.isFinite(share) || share <= 0) {
    return `[${empty.repeat(width)}]`;
  }

  const filled = Math.max(0, Math.min(width, Math.round(share * width)));
  return `[${full.repeat(filled)}${empty.repeat(width - filled)}]`;
}

export function buildTerminalSummary(baseline, candidate, candidatePerformance, structuralResult, performanceResult, allFailures, stream) {
  const lines = [];
  const colorEnabled = Boolean(stream?.isTTY && !process.env.NO_COLOR);
  const unicodeBars = supportsUnicode(stream);
  const isSuccess = allFailures.length === 0;
  const statusText = isSuccess ? green('PASS', colorEnabled) : red('FAIL', colorEnabled);

  lines.push(bold('Trace Contract Report', colorEnabled));
  lines.push(`${statusText}  structural=${structuralResult.failures.length}  performance=${performanceResult.failures.length}`);
  lines.push(dim(`baseline descriptors=${baseline.traceDescriptors.size}  candidate=${candidate.traceDescriptors.size}  events=${candidate.totalJsonLines}`, colorEnabled));

  const SECTIONS = [
    { title: 'DB Operations', categories: ['db'], colorFn: (s, c) => cyan(s, c) },
    { title: 'Ledger', categories: ['ledger'], colorFn: (s, c) => green(s, c) },
    { title: 'Consensus', categories: ['consensus'], colorFn: (s, c) => s },
    { title: 'Network & Peer', categories: ['network', 'peer_wait'], colorFn: (s, c) => s },
  ];

  for (const section of SECTIONS) {
    const stats = (candidatePerformance?.descriptorStats ?? [])
      .filter(stat => section.categories.includes(stat.category))
      .sort((a, b) => {
        const aMean = safeNumber(a.total?.meanMs, -1);
        const bMean = safeNumber(b.total?.meanMs, -1);
        if (bMean !== aMean) return bMean - aMean;
        return safeNumber(b.eventCount, 0) - safeNumber(a.eventCount, 0);
      });
    const timedStats = stats.filter(stat => stat.total != null);
    const untimedCount = stats.length - timedStats.length;

    lines.push('');
    lines.push(bold(section.title, colorEnabled));

    if (timedStats.length === 0) {
      if (untimedCount > 0) {
        lines.push(dim(`  no timed data (${untimedCount} untimed descriptors omitted)`, colorEnabled));
        continue;
      }

      lines.push(dim('  no data', colorEnabled));
      continue;
    }

    const maxMean = timedStats.reduce((m, s) => Math.max(m, safeNumber(s.total?.meanMs, 0)), 0);

    for (const stat of timedStats) {
      const relShare = maxMean > 0 ? safeNumber(stat.total.meanMs, 0) / maxMean : 0;
      const bar = formatShareBar(relShare, 16, unicodeBars);
      const label = padText(formatDescriptorLabel(stat, 46), 48);
      const row = `  ${bar}  ${label}  ${padText(stat.total.sampleCount + ' calls', 12)}  mean ${padText(formatMs(stat.total.meanMs), 11)}  p95 ${padText(formatMs(stat.total.p95Ms), 11)}  max ${padText(formatMs(stat.total.maxMs), 9)}  outliers ${formatPercent(stat.total.outlierRate)}`;
      lines.push(section.colorFn(row, colorEnabled));
    }

    if (untimedCount > 0) {
      lines.push(dim(`  + ${untimedCount} untimed descriptors omitted`, colorEnabled));
    }
  }

  if (allFailures.length > 0) {
    lines.push('');
    lines.push(bold('Regressions', colorEnabled));

    for (const failure of allFailures.slice(0, 10)) {
      if (failure.kind === 'missing-trace-descriptor') {
        lines.push(red(`- missing ${formatDescriptorLabel(failure.traceDescriptor, 96)}`, colorEnabled));
        continue;
      }

      if (failure.kind === 'sequence-order-changed') {
        lines.push(red(`- sequence changed: ${formatDescriptorLabel(failure.before, 42)} before ${formatDescriptorLabel(failure.after, 42)}`, colorEnabled));
        continue;
      }

      if (failure.kind === 'metric-regression') {
        const scope = failure.scope === 'category'
          ? `category ${failure.category}`
          : formatDescriptorLabel(failure, 72);
        lines.push(red(`- ${scope} ${failure.metric}: ${failure.baseline} -> ${failure.candidate} (${failure.ratio}x > ${failure.ratioThreshold}x)`, colorEnabled));
        continue;
      }

      if (failure.kind === 'outlier-regression') {
        const scope = failure.scope === 'category'
          ? `category ${failure.category}`
          : formatDescriptorLabel(failure, 72);
        lines.push(red(`- ${scope} outlier rate: ${failure.baselineOutlierRate} -> ${failure.candidateOutlierRate} (delta ${failure.delta} > ${failure.deltaThreshold})`, colorEnabled));
      }
    }
  }

  return `${lines.join('\n')}\n`;
}

function resolveSummaryStream(summaryPath) {
  if (summaryPath === '/dev/stdout') {
    return process.stdout;
  }

  if (summaryPath === '/dev/stderr') {
    return process.stderr;
  }

  return null;
}

export function writeSummary(summaryPath, markdownSummary, terminalSummary) {
  if (!summaryPath || typeof summaryPath !== 'string' || summaryPath.trim() === '') {
    return;
  }

  const stream = resolveSummaryStream(summaryPath);
  if (stream) {
    stream.write(stream.isTTY ? terminalSummary : markdownSummary);
    return;
  }

  appendFileSync(summaryPath, markdownSummary);
}

export function buildSummary(baseline, candidate, structuralResult, performanceResult, allFailures) {
  const lines = [];
  const isSuccess = allFailures.length === 0;
  const structuralFailures = structuralResult.failures.length;
  const performanceFailures = performanceResult.failures.length;

  lines.push('## Trace Contract Report');
  lines.push('');
  lines.push(`- Status: ${isSuccess ? 'PASS' : 'FAIL'}`);
  lines.push(`- Structural failures: ${structuralFailures}`);
  lines.push(`- Performance regressions: ${performanceFailures}`);
  lines.push(`- Baseline trace descriptors: ${baseline.traceDescriptors.size}`);
  lines.push(`- Candidate trace descriptors: ${candidate.traceDescriptors.size}`);
  lines.push(`- Candidate json events: ${candidate.totalJsonLines}`);
  lines.push('');

  if (performanceResult.categoryComparisons.length > 0) {
    lines.push('### Category latency (total ms)');
    lines.push('');
    lines.push('| Category | Samples (base/cand) | Mean (base/cand) | P95 (base/cand) | P95 ratio |');
    lines.push('| --- | ---: | ---: | ---: | ---: |');

    for (const row of performanceResult.categoryComparisons) {
      lines.push(
        `| ${row.category} | ${row.baselineSamples}/${row.candidateSamples} | ${roundTo(row.baselineMeanMs, 3)}/${roundTo(row.candidateMeanMs, 3)} | ${roundTo(row.baselineP95Ms, 3)}/${roundTo(row.candidateP95Ms, 3)} | ${row.p95Ratio ?? 'n/a'} |`,
      );
    }

    lines.push('');
  }

  const topDescriptors = performanceResult.descriptorComparisons.slice(0, 15);
  if (topDescriptors.length > 0) {
    lines.push('### Top trace descriptors by candidate P95 (total ms)');
    lines.push('');
    lines.push('| Trace descriptor | Category | Samples (base/cand) | Mean (base/cand) | P95 (base/cand) | Outlier rate (base/cand) |');
    lines.push('| --- | --- | ---: | ---: | ---: | ---: |');

    for (const row of topDescriptors) {
      lines.push(
        `| ${formatTraceDescriptor(row)} | ${row.category} | ${row.baselineSamples}/${row.candidateSamples} | ${roundTo(row.baselineMeanMs, 3)}/${roundTo(row.candidateMeanMs, 3)} | ${roundTo(row.baselineP95Ms, 3)}/${roundTo(row.candidateP95Ms, 3)} | ${roundTo(row.baselineOutlierRate, 3)}/${roundTo(row.candidateOutlierRate, 3)} |`,
      );
    }

    lines.push('');
  }

  if (allFailures.length > 0) {
    lines.push('### Regression details');
    lines.push('');
    for (const failure of allFailures.slice(0, 30)) {
      if (failure.kind === 'missing-trace-descriptor') {
        lines.push(`- missing trace descriptor: ${formatTraceDescriptor(failure.traceDescriptor)}`);
        continue;
      }

      if (failure.kind === 'sequence-order-changed') {
        lines.push(`- sequence changed: ${formatTraceDescriptor(failure.before)} should be before ${formatTraceDescriptor(failure.after)}`);
        continue;
      }

      if (failure.kind === 'metric-regression') {
        const scope = failure.scope === 'category'
          ? `category ${failure.category}`
          : formatTraceDescriptor(failure);
        lines.push(
          `- ${scope} ${failure.metric} regression: baseline=${failure.baseline}, candidate=${failure.candidate}, ratio=${failure.ratio}, threshold=${failure.ratioThreshold}`,
        );
        continue;
      }

      if (failure.kind === 'outlier-regression') {
        const scope = failure.scope === 'category'
          ? `category ${failure.category}`
          : formatTraceDescriptor(failure);
        lines.push(
          `- ${scope} outlier-rate regression: baseline=${failure.baselineOutlierRate}, candidate=${failure.candidateOutlierRate}, delta=${failure.delta}, threshold=${failure.deltaThreshold}`,
        );
      }
    }

    lines.push('');
  }

  return `${lines.join('\n')}\n`;
}

export function buildFailureReport(baseline, candidate, structuralResult, performanceResult, allFailures) {
  const baselineSummary = baseline.source && typeof baseline.source === 'object'
    ? {
      file: typeof baseline.source.file === 'string' ? baseline.source.file : baseline.filePath,
      jsonEvents: Number.isInteger(baseline.source.jsonEvents)
        ? baseline.source.jsonEvents
        : baseline.totalJsonLines,
      traceDescriptorCount: Number.isInteger(baseline.source.traceDescriptorCount)
        ? baseline.source.traceDescriptorCount
        : baseline.traceDescriptors.size,
    }
    : {
      file: baseline.filePath,
      jsonEvents: baseline.totalJsonLines,
      traceDescriptorCount: baseline.traceDescriptors.size,
    };

  return {
    status: 'fail',
    baseline: baselineSummary,
    candidate: {
      file: candidate.filePath,
      jsonEvents: candidate.totalJsonLines,
      traceDescriptorCount: candidate.traceDescriptors.size,
    },
    structural: {
      failures: structuralResult.failures.map(serializeFailure),
      additions: structuralResult.additions.map(formatTraceDescriptor),
    },
    performance: {
      failures: performanceResult.failures.map(serializeFailure),
      categoryComparisons: performanceResult.categoryComparisons,
      topDescriptorComparisons: performanceResult.descriptorComparisons.slice(0, 50),
    },
    failures: allFailures.map(serializeFailure),
  };
}