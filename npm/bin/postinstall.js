#!/usr/bin/env node
import { ensureBinary } from "./runtime.js";

try {
  const binaryPath = await ensureBinary();
  console.error(`nuwax-harness ready at ${binaryPath}`);
} catch (error) {
  // Installation remains usable offline: the launcher retries on first use.
  // An incomplete archive/cache is never reported as ready.
  console.error(`nuwax-harness postinstall: ${error.message}`);
}
