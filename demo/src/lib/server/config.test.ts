import { describe, expect, it } from 'vitest';
import { loadEngineConfig } from './config';

describe('loadEngineConfig', () => {
  it('uses localhost defaults when engine urls are not configured', () => {
    const config = loadEngineConfig({});

    expect(config.surch.url).toBe('http://127.0.0.1:7700');
    expect(config.opensearch.url).toBe('http://127.0.0.1:9200');
  });

  it('rejects unsupported protocols', () => {
    expect(() =>
      loadEngineConfig({
        SURCH_URL: 'file:///etc/passwd'
      })
    ).toThrow(/SURCH_URL/);
  });

  it('rejects urls with credentials or search params', () => {
    expect(() =>
      loadEngineConfig({
        OPENSEARCH_URL: 'http://user:pass@localhost:9200'
      })
    ).toThrow(/credentials/);

    expect(() =>
      loadEngineConfig({
        OPENSEARCH_URL: 'http://localhost:9200/?target=http://example.com'
      })
    ).toThrow(/query/);
  });
});
