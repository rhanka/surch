import { describe, expect, it } from 'vitest';
import { parseBanCsvText } from './csv';

const tinyCsv = `id,numero,rep,nom_voie,code_postal,code_insee,nom_commune,lon,lat
75101_0001_00001,1,,Rue de Rivoli,75001,75101,Paris 1er Arrondissement,2.3364,48.8609
33063_0002_00010B,10,B,Cours de l'Intendance,33000,33063,Bordeaux,-0.5792,44.8412
`;

const officialTinyCsv = `id;id_fantoir;numero;rep;nom_voie;code_postal;code_insee;nom_commune;x;y;lon;lat
75103_ka7f7y_00001;;1;;Voie B/3;75003;75103;Paris 3e Arrondissement;652996.66;6862232.89;2.359369;48.858416
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

  it('parses official BAN semicolon CSV rows', async () => {
    const documents = await parseBanCsvText(officialTinyCsv, { sourceName: 'official.csv' });

    expect(documents).toHaveLength(1);
    expect(documents[0]).toMatchObject({
      id: '75103_ka7f7y_00001',
      label: '1 Voie B/3 75003 Paris 3e Arrondissement',
      location: {
        lat: 48.858416,
        lon: 2.359369
      }
    });
  });
});
