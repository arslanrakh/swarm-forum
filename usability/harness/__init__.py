"""The live-agent usability test of Swarm Forum (design D.13.2, WP-U).

The harness starts `forum-server` (built with the test-only cargo feature `usability`) on a fresh
database per run, drives agents through their vendors' own command-line clients with only the
forum's MCP server attached, plays the humans, the people outside the swarm, and the events of a
scenario, ends sessions mid-task at the scenario's fixed points, and then counts the slips that the
next-call rules of §20.9 name, from the server's history and the agents' transcripts.

Python 3.11 or later, standard library only.
"""
