# -*- coding: utf-8 -*-
"""Prépare le manifeste de mise à jour signé de ForgePlayer (ne publie rien).

    python outils/publier.py cle                      génère la paire Ed25519 (une seule fois)
    python outils/publier.py [--notes "Phrase."]      calcule taille + SHA-256 de
        dist/ForgePlayer_v<version>_Setup.exe et écrit dist/latest.json + latest.json.sig

La clé privée (jamais dans le dépôt) : $FORGEPLAYER_UPDATER_KEY, sinon $MDP_DIR/forgeplayer-updater.key. La clé
publique correspondante est embarquée dans crates/forgeplayer/src/updater.rs (PUBLIC_KEY).
Ensuite, à la main : attacher Setup + latest.json + latest.json.sig à la release vX.Y.Z.
"""

import argparse
import base64
import hashlib
import json
import os
import re
import sys
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

RACINE = Path(__file__).resolve().parents[1]
CLE = None  # fixé par main() via chemin_cle()
DEPOT = "Heiphaistos/ForgePlayer"


def chemin_cle():
    e = os.environ.get("FORGEPLAYER_UPDATER_KEY")
    if e:
        return Path(e)
    d = os.environ.get("MDP_DIR")
    if d:
        return Path(d) / "forgeplayer-updater.key"
    sys.exit("FORGEPLAYER_UPDATER_KEY (ou MDP_DIR) non défini : chemin de la clé de signature")


def b64(raw):
    return base64.b64encode(raw).decode()


def charger():
    return Ed25519PrivateKey.from_private_bytes(base64.b64decode(CLE.read_text().strip()))


def publique(cle):
    return b64(cle.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw))


def version():
    return re.search(r'^version\s*=\s*"([^"]+)"', (RACINE / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("action", nargs="?", default="manifeste", choices=["manifeste", "cle"])
    p.add_argument("--notes", default="")
    a = p.parse_args()
    global CLE
    CLE = chemin_cle()
    if a.action == "cle":
        if CLE.exists():
            sys.exit("clé déjà présente : %s (ne pas écraser)" % CLE)
        cle = Ed25519PrivateKey.generate()
        seed = cle.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
        CLE.write_text(b64(seed))
        print("clé privée : %s\nclé publique (PUBLIC_KEY dans updater.rs) : %s" % (CLE, publique(cle)))
        return
    if not CLE.exists():
        sys.exit("clé de signature absente : %s (python outils/publier.py cle)" % CLE)
    v = version()
    nom = "ForgePlayer_v%s_Setup.exe" % v
    setup = RACINE / "dist" / nom
    if not setup.exists():
        sys.exit("installateur introuvable : %s (compiler installer/ForgePlayer.iss)" % setup)
    donnees = setup.read_bytes()
    manifeste = json.dumps({
        "version": v,
        "notes": a.notes,
        "url": "https://github.com/%s/releases/download/v%s/%s" % (DEPOT, v, nom),
        "size": len(donnees),
        "sha256": hashlib.sha256(donnees).hexdigest(),
    }, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    cle = charger()
    (RACINE / "dist" / "latest.json").write_bytes(manifeste)
    (RACINE / "dist" / "latest.json.sig").write_text(b64(cle.sign(manifeste)))
    print("OK dist/latest.json + latest.json.sig (clé publique %s)" % publique(cle))
    print("À joindre à la release v%s : %s, latest.json, latest.json.sig" % (v, nom))


main()
