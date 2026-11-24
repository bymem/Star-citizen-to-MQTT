sc-recorder
===========

Records Star Citizen's local telemetry so the SC MQTT Companion can be
developed without the game. It is read-only: it never changes the game,
its files or your settings, and it only talks to 127.0.0.1:2942.


1. Enable telemetry (once)
--------------------------

Open (or create) user.cfg in your Star Citizen channel folder, for example:

    C:\Program Files\Roberts Space Industries\StarCitizen\LIVE\user.cfg

Add this line, save, and restart the game:

    server.game.telemetry.enabled=1


2. Record
---------

Double-click sc-recorder.exe. You can start it before or after the game;
it waits for the game and records as soon as your character has spawned.

Play for 10-15 minutes and try to cover:
  - walking around on foot
  - flying a ship, quantum travel, landing
  - combat, if possible
  - sitting in a turret seat, if you want turret data

Then press Ctrl+C or close the window.


3. Send it back
---------------

A file named sc-recording-<date>-<time>.zip is created next to the exe.
Send that zip. It may contain your aUEC balance, ship names and your
Windows user name in a file path, so share it privately.

Windows SmartScreen may warn because the exe is not signed yet. Choose
"More info" and then "Run anyway".
