import { demoQueries } from '$lib/demoQueries';
import { loadEngineConfig } from '$lib/server/config';
import { getBanTinyFixture } from '$lib/server/fixture';

export function load() {
  const fixture = getBanTinyFixture();
  const engines = loadEngineConfig();

  return {
    fixture,
    engines: [engines.surch, engines.opensearch],
    queries: demoQueries
  };
}
