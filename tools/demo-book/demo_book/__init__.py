"""The generated demo book (EKOS RFC 0218): a tour of the console into a capture, a book from the capture."""

import sys
from pathlib import Path

# The manual-section and table helpers are lakehouse-docs' (RFC 0208), so the book and the pipeline
# documentation keep notes the same way and publish through the same publishers.
_DOCS = Path(__file__).resolve().parents[2] / "lakehouse-docs"
if str(_DOCS) not in sys.path:
    sys.path.insert(0, str(_DOCS))
