import { describe, expect, it } from 'vitest';
import { parseBanCsvText } from './csv';

const tinyCsv = `id,numero,rep,nom_voie,code_postal,code_insee,nom_commune,lon,lat
75101_0001_00001,1,,Rue de Rivoli,75001,75101,Paris 1er Arrondissement,2.3364,48.8609
33063_0002_00010B,10,B,Cours de l'Intendance,33000,33063,Bordeaux,-0.5792,44.8412
`;

describe('BAN CSV parser', () => {
  it('parses tiny BAN CSV rows into demo documents', async () => {
    const documents = await parseBanCsvText(tinyCsv, { sourceName: 'unit.csv' });

    expect(documents).toHaveLength(2);
    expect(documents[0]).toEqual({
      city_code: '75101',
      city_name: 'Paris 1er Arrondissement',
      house_number: '1',
      id: '75101_0001_00001',
      label: '1 Rue de Rivoli 75001 Paris 1er Arrondissement',
      location: {
        lat: 48.8609,
        lon: 2.3364
      },
      postcode: '75001',
      source: 'BAN',
      street_name: 'Rue de Rivoli'
    });
    expect(documents[1].house_number).toBe('10B');
  });
});
