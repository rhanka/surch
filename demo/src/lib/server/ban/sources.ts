export const BAN_CSV_DIRECTORY_URL = 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv';

export type BanDownloadProfile = {
  id: 'paris' | 'france';
  label: string;
  fileName: string;
  url: string;
  expectedBytes: number;
};

export const BAN_DOWNLOAD_PROFILES: Record<BanDownloadProfile['id'], BanDownloadProfile> = {
  paris: {
    id: 'paris',
    label: 'BAN Paris departmental sample',
    fileName: 'adresses-75.csv.gz',
    url: `${BAN_CSV_DIRECTORY_URL}/adresses-75.csv.gz`,
    expectedBytes: 3_764_435
  },
  france: {
    id: 'france',
    label: 'BAN France full dataset',
    fileName: 'adresses-france.csv.gz',
    url: `${BAN_CSV_DIRECTORY_URL}/adresses-france.csv.gz`,
    expectedBytes: 922_089_539
  }
};

export function banDownloadProfile(id: string | undefined): BanDownloadProfile {
  if (id === 'france') {
    return BAN_DOWNLOAD_PROFILES.france;
  }

  return BAN_DOWNLOAD_PROFILES.paris;
}
