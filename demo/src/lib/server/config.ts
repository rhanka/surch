import type { EngineConfig } from '$lib/types';

type EnvSource = Partial<Record<'SURCH_URL' | 'OPENSEARCH_URL', string | undefined>>;

const DEFAULT_SURCH_URL = 'http://127.0.0.1:7700';
const DEFAULT_OPENSEARCH_URL = 'http://127.0.0.1:9200';

export function loadEngineConfig(env: EnvSource = process.env): {
  surch: EngineConfig;
  opensearch: EngineConfig;
} {
  const surchRaw = env.SURCH_URL;
  const opensearchRaw = env.OPENSEARCH_URL;

  return {
    surch: {
      id: 'surch',
      label: 'Surch',
      url: parseEngineUrl('SURCH_URL', surchRaw ?? DEFAULT_SURCH_URL),
      configured: Boolean(surchRaw)
    },
    opensearch: {
      id: 'opensearch',
      label: 'OpenSearch',
      url: parseEngineUrl('OPENSEARCH_URL', opensearchRaw ?? DEFAULT_OPENSEARCH_URL),
      configured: Boolean(opensearchRaw)
    }
  };
}

function parseEngineUrl(name: string, value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${name} must be an absolute http(s) URL`);
  }

  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`${name} must use http or https`);
  }

  if (url.username || url.password) {
    throw new Error(`${name} must not include credentials`);
  }

  if (url.search) {
    throw new Error(`${name} must not include query parameters`);
  }

  if (url.hash) {
    throw new Error(`${name} must not include a fragment`);
  }

  if (!url.hostname || url.hostname.length > 253) {
    throw new Error(`${name} must include a valid host`);
  }

  if (url.pathname !== '/' && url.pathname.endsWith('/')) {
    url.pathname = url.pathname.slice(0, -1);
  }

  return url.toString().replace(/\/$/, '');
}
