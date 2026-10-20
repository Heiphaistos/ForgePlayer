"""Sans FORGEPLAYER_UPDATER_KEY ni MDP_DIR, publier.py refuse avec un message clair."""
import os, subprocess, sys, unittest

ICI = os.path.dirname(os.path.abspath(__file__))


class SansCle(unittest.TestCase):
    def test_refus_clair(self):
        env = {k: v for k, v in os.environ.items() if k not in ("FORGEPLAYER_UPDATER_KEY", "MDP_DIR")}
        r = subprocess.run([sys.executable, os.path.join(ICI, "publier.py")],
                           env=env, capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("FORGEPLAYER_UPDATER_KEY", r.stderr)


if __name__ == "__main__":
    unittest.main()
