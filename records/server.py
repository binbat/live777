import http.server
import socketserver
import sys
from pathlib import Path
import os


class CORSRequestHandler(http.server.SimpleHTTPRequestHandler):
    """SimpleHTTPRequestHandler with Cross-Origin Resource Sharing (CORS) support.

    Only requests from https://reference.dashif.org are allowed to satisfy the dash.js reference player requirements.
    """

    # Allow handling of preflight (OPTIONS) requests
    def do_OPTIONS(self):  # noqa: N802  # Keep parent class naming style
        self.send_response(204)  # 204 No Content
        self.send_cors_headers()
        self.end_headers()

    # Insert CORS headers when sending response headers
    def end_headers(self):  # noqa: D401
        self.send_cors_headers()
        super().end_headers()

    def send_cors_headers(self):
        """Write the CORS headers consistently."""
        self.send_header("Access-Control-Allow-Origin", "https://reference.dashif.org")
        self.send_header("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS")
        # Allow custom request headers from the frontend (Range is particularly important for dash.js)
        self.send_header(
            "Access-Control-Allow-Headers", "Range, If-Modified-Since, Cache-Control"
        )
        # Expose specific response headers to the frontend
        self.send_header(
            "Access-Control-Expose-Headers", "Content-Length, Content-Range"
        )


if __name__ == "__main__":
    # Default listening port is 8000, can be overridden via command line arguments
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000

    # Switch to the records directory (the script directory) to ensure the correct root path
    records_dir = Path(__file__).resolve().parent
    # Ensure the HTTP server serves files from the records directory
    os.chdir(records_dir)

    print(f"Serving directory: {records_dir}")
    print("Press Ctrl+C to stop the server.")

    # Create and start the HTTP server
    with socketserver.TCPServer(("", port), CORSRequestHandler) as httpd:
        try:
            print(f"🚀 HTTP server is running on port {port} (CORS enabled).")
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nShutting down server…")
        finally:
            httpd.server_close()
