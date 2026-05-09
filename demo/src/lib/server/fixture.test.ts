import { describe, expect, it } from 'vitest';
import { getBanTinyFixture } from './fixture';

describe('getBanTinyFixture', () => {
  it('loads the BAN tiny fixture as deterministic documents', () => {
    const fixture = getBanTinyFixture();

    expect(fixture.name).toBe('ban_tiny');
    expect(fixture.documents).toHaveLength(3);
    expect(fixture.documents.map((document) => document.id)).toEqual([
      '75101_0001_00001',
      '33063_0002_00010B',
      '67482_0003_00007'
    ]);
  });
});
