import { json } from '@sveltejs/kit';
import { loadEngineConfig } from '$lib/server/config';
import type { RequestHandler } from './$types';

export const GET = (() => {
  const config = loadEngineConfig();

  return json({
    modes: ['surch', 'opensearch', 'compare'],
    engines: [config.surch, config.opensearch]
  });
}) satisfies RequestHandler;
