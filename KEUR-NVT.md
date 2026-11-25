# KEUR-NVT

Stukken die de documentatiestandaard noemt en die in deze repository bewust
ontbreken, met de reden. Een regel per stuk: `module: reden`.

about.hbs: er is geen about.hbs, want er wordt geen licentieoverzicht van afhankelijkheden gegenereerd.
about.toml: er is geen about.toml, want cargo-about wordt niet gebruikt.
docs: er is geen aparte documentatiemap; alles staat in README.md, QUALITY.md en CHANGELOG.md.
samples: voorbeeldinvoer staat in de map examples, een aparte samples-map is er niet.
scripts: er zijn geen hulpscripts; bouwen en testen gaat met cargo.
.github/workflows/release.yml: er is geen releaseworkflow, want de tool is nog niet gepubliceerd en er worden geen binaries verspreid.
