package com.kvandeh.angelo.sonar;

import org.sonar.api.Plugin;

/** Registers the mutation metrics and the sensor that fills them in. */
public class AngeloPlugin implements Plugin {

  @Override
  public void define(Context context) {
    context.addExtensions(AngeloMetrics.class, MutationScoreSensor.class);
  }
}
