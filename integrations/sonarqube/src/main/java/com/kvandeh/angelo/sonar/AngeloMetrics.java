package com.kvandeh.angelo.sonar;

import java.util.Arrays;
import java.util.List;
import org.sonar.api.measures.Metric;
import org.sonar.api.measures.Metrics;

/**
 * The metrics SonarQube has no import path for.
 *
 * <p>Generic issue import creates issues; a percentage is a measure, and
 * {@code api/custom_measures} was removed in 8.2. Declaring {@link Metrics} is
 * the only remaining way to put a real number on the overview, with history and
 * a quality-gate condition on it.
 */
public class AngeloMetrics implements Metrics {

  public static final String DOMAIN = "Mutation Testing";

  /** detected / valid, exactly as Angelo's own Summary::score computes it. */
  public static final Metric<Double> MUTATION_SCORE =
      new Metric.Builder("angelo_mutation_score", "Mutation Score", Metric.ValueType.PERCENT)
          .setDescription("Percentage of planted faults the test suite detected")
          .setDirection(Metric.DIRECTION_BETTER)
          .setQualitative(true)
          .setDomain(DOMAIN)
          .setBestValue(100.0)
          .setWorstValue(0.0)
          .create();

  /** The denominator, so the score is never read without its sample size. */
  public static final Metric<Integer> MUTANTS_VALID =
      new Metric.Builder("angelo_mutants_valid", "Mutants Scored", Metric.ValueType.INT)
          .setDescription("Mutants that counted toward the score")
          .setDirection(Metric.DIRECTION_NONE)
          .setQualitative(false)
          .setDomain(DOMAIN)
          .create();

  public static final Metric<Integer> MUTANTS_SURVIVED =
      new Metric.Builder("angelo_mutants_survived", "Mutants Survived", Metric.ValueType.INT)
          .setDescription("Faults no test noticed")
          .setDirection(Metric.DIRECTION_WORST)
          .setQualitative(true)
          .setDomain(DOMAIN)
          .setBestValue(0.0)
          .create();

  /**
   * Published on purpose. A run where everything errored has no score at all,
   * and an empty issue list renders as a clean bill of health, so the count that
   * explains it has to be visible next to the number.
   */
  public static final Metric<Integer> MUTANTS_ERRORED =
      new Metric.Builder("angelo_mutants_errored", "Mutants Errored", Metric.ValueType.INT)
          .setDescription("Mutants no run could judge; usually a broken test command")
          .setDirection(Metric.DIRECTION_WORST)
          .setQualitative(false)
          .setDomain(DOMAIN)
          .setBestValue(0.0)
          .create();

  @Override
  public List<Metric> getMetrics() {
    return Arrays.asList(MUTATION_SCORE, MUTANTS_VALID, MUTANTS_SURVIVED, MUTANTS_ERRORED);
  }
}
