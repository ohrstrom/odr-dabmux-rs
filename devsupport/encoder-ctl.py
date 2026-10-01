# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "supervisor",
# ]
# ///

import argparse
import xmlrpc.client

from supervisor.xmlrpc import SupervisorTransport


SERVER_URL = "http://localhost:18900"


def get_server():
    transport = SupervisorTransport(None, None, SERVER_URL)
    return xmlrpc.client.ServerProxy(
        "http://127.0.0.1",
        transport=transport,
    )


def status(server):
    for process in server.supervisor.getAllProcessInfo():
        print(
            f"{process['name']:<20} "
            f"{process['statename']:<10} "
            f"pid={process['pid']}"
        )


def reread(server):
    added, changed, removed = server.supervisor.reloadConfig()[0]

    if added:
        print("added:", ", ".join(added))
    if changed:
        print("changed:", ", ".join(changed))
    if removed:
        print("removed:", ", ".join(removed))

    if not any((added, changed, removed)):
        print("No config updates")


def update(server):
    added, changed, removed = server.supervisor.reloadConfig()[0]

    for name in removed:
        print(f"removing {name}")
        server.supervisor.stopProcessGroup(name)
        server.supervisor.removeProcessGroup(name)

    for name in changed:
        print(f"updating {name}")
        server.supervisor.stopProcessGroup(name)
        server.supervisor.removeProcessGroup(name)
        server.supervisor.addProcessGroup(name)

    for name in added:
        print(f"adding {name}")
        server.supervisor.addProcessGroup(name)

    if not any((added, changed, removed)):
        print("No config updates")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "command",
        choices=["status", "reread", "update"],
    )
    args = parser.parse_args()

    server = get_server()

    match args.command:
        case "status":
            status(server)
        case "reread":
            reread(server)
        case "update":
            update(server)


if __name__ == "__main__":
    main()