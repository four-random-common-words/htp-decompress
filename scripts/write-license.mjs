// Writes npm/LICENSE as the GPL text followed by THIRD-PARTY-NOTICES.md, and copies THIRD-PARTY-NOTICES.md to npm/.
// License extractors such as Angular's take only the first file named like LICENSE from each package, so the notices
// that the bundled crates' licenses require must be in that file to reach an app's list of licenses.
import { readFileSync, writeFileSync } from 'node:fs';

const ROOT = new URL('../', import.meta.url);

const gpl = readFileSync(new URL('LICENSE', ROOT), 'utf8');
const notices = readFileSync(new URL('THIRD-PARTY-NOTICES.md', ROOT), 'utf8');

writeFileSync(new URL('npm/LICENSE', ROOT), `${gpl}\n${'='.repeat(80)}\n\n${notices}`);
writeFileSync(new URL('npm/THIRD-PARTY-NOTICES.md', ROOT), notices);
