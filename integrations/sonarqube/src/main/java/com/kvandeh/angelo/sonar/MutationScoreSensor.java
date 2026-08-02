package com.kvandeh.angelo.sonar;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.File;
import java.io.IOException;
import java.io.Reader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;
import org.sonar.api.batch.sensor.Sensor;
import org.sonar.api.batch.sensor.SensorContext;
import org.sonar.api.batch.sensor.SensorDescriptor;
import org.sonar.api.utils.log.Logger;
import org.sonar.api.utils.log.Loggers;

/**
 * Reads the mutation-testing-report file Angelo writes with {@code --report}
 * and publishes the score as a project measure.
 *
 * <p>The file is the cross-language schema StrykerJS, Stryker.NET and Stryker4s
 * also emit, so this sensor is not Angelo-specific by accident of format.
 */
public class MutationScoreSensor implements Sensor {

  private static final Logger LOG = Loggers.get(MutationScoreSensor.class);

  public static final String REPORT_PATH_KEY = "sonar.angelo.reportPath";
  private static final String DEFAULT_REPORT_PATH = "angelo.json";

  @Override
  public void describe(SensorDescriptor descriptor) {
    descriptor.name("Angelo mutation report");
  }

  @Override
  public void execute(SensorContext context) {
    String configured = context.config().get(REPORT_PATH_KEY).orElse(DEFAULT_REPORT_PATH);
    File report = context.fileSystem().resolvePath(configured);
    if (!report.isFile()) {
      LOG.info("No Angelo report at {}, skipping mutation metrics", report);
      return;
    }

    Tally tally;
    try {
      tally = read(report);
    } catch (IOException | RuntimeException failure) {
      // A malformed report must not fail the whole analysis: the mutation score
      // is extra information, not the reason anybody ran the scanner.
      LOG.warn("Could not read the Angelo report at {}: {}", report, failure.getMessage());
      return;
    }

    context
        .<Integer>newMeasure()
        .forMetric(AngeloMetrics.MUTANTS_SURVIVED)
        .on(context.project())
        .withValue(tally.survived)
        .save();
    context
        .<Integer>newMeasure()
        .forMetric(AngeloMetrics.MUTANTS_ERRORED)
        .on(context.project())
        .withValue(tally.errored)
        .save();
    context
        .<Integer>newMeasure()
        .forMetric(AngeloMetrics.MUTANTS_VALID)
        .on(context.project())
        .withValue(tally.valid())
        .save();

    if (tally.valid() == 0) {
      // No score is not a zero score, and it is certainly not a pass. Publishing
      // 0% here would read as a catastrophic suite rather than as a run that
      // measured nothing.
      LOG.warn(
          "Angelo scored no mutants ({} errored): publishing no mutation score", tally.errored);
      return;
    }

    double score = 100.0 * tally.detected() / tally.valid();
    context
        .<Double>newMeasure()
        .forMetric(AngeloMetrics.MUTATION_SCORE)
        .on(context.project())
        .withValue(score)
        .save();
    LOG.info(
        "Angelo mutation score {}% ({}/{} detected)",
        String.format("%.1f", score), tally.detected(), tally.valid());
  }

  private static Tally read(File report) throws IOException {
    Tally tally = new Tally();
    try (Reader reader = Files.newBufferedReader(report.toPath(), StandardCharsets.UTF_8)) {
      JsonObject root = JsonParser.parseReader(reader).getAsJsonObject();
      JsonObject files = root.getAsJsonObject("files");
      if (files == null) {
        return tally;
      }
      for (Map.Entry<String, JsonElement> file : files.entrySet()) {
        JsonElement mutants = file.getValue().getAsJsonObject().get("mutants");
        if (mutants == null) {
          continue;
        }
        for (JsonElement mutant : mutants.getAsJsonArray()) {
          JsonElement status = mutant.getAsJsonObject().get("status");
          if (status != null) {
            tally.count(status.getAsString());
          }
        }
      }
    }
    return tally;
  }

  /**
   * The schema's own arithmetic: detected is Killed plus Timeout, and valid
   * excludes RuntimeError and Ignored. That is Angelo's {@code Summary::score}
   * character for character, so this recomputes nothing it could disagree with.
   */
  private static final class Tally {
    private final Map<String, Integer> counts = new HashMap<>();
    private int survived;
    private int errored;

    void count(String status) {
      counts.merge(status, 1, Integer::sum);
      if ("Survived".equals(status) || "NoCoverage".equals(status)) {
        survived++;
      } else if ("RuntimeError".equals(status)) {
        errored++;
      }
    }

    int detected() {
      return counts.getOrDefault("Killed", 0) + counts.getOrDefault("Timeout", 0);
    }

    int valid() {
      return detected() + survived;
    }
  }
}
