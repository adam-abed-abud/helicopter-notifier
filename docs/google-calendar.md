# Google Calendar Setup

Helicopter reads Google Calendar through the Google Calendar API. Browser login alone is not enough for a desktop app; the app needs OAuth desktop credentials once.

1. Create an OAuth client in Google Cloud Console:
   - Application type: Desktop app
   - API: Google Calendar API
   - Scope used by this app: `https://www.googleapis.com/auth/calendar.readonly`

2. Download the OAuth client JSON and save it as:

   ```text
   ~/.config/helicopter/credentials.json
   ```

3. Start the app:

   ```bash
   helicopter
   ```

4. On first run, Helicopter opens a browser consent page. After approval it stores:

   ```text
   ~/.config/helicopter/token.json
   ```

The app then polls your primary calendar and uses the next timed event title as the helicopter banner text. All-day events are ignored.
