# Changelog

## 0.9.1 (2026-10-22)
- The TV remote works in Chillarr on LG TVs. Arrows and OK did nothing before: LG's Jellyfin app never handed the remote to Chillarr's page, and older TVs send the keys in a form Chillarr didn't read. Now the arrows move from the first poster and OK opens it.
- Every TV has its own phone remote code. On the TV, open Settings > Phone remote, then type that code on your phone. The phone then controls that TV and only that TV. Each TV keeps its code until you press New code on it. The code in Chillarr on the PC still controls the PC.
- The phone tells you which screen it's controlling, and says so if that TV isn't showing Chillarr right now.
- The phone remote works over Tailscale too. Settings lists your Tailscale address next to the Wi-Fi one, and opening the remote by the Tailscale name no longer gets turned away.
- On the PC, Settings > General > Phone remote lists each TV with its code, and Forget turns one off.

## 0.9.0 (2026-10-21)
- Chillarr now lives inside Jellyfin. Open Jellyfin in a browser, on your phone (the Jellyfin app) or in Jellyfin Media Player, sign in, and you land on Chillarr's home instead of Jellyfin's: the same rows, Top 10, hover cards with trailers, filters, search, My List, Download, Watch now and the player with night mode, subtitle download and sync, skip intro and Up next. Everyone gets their own My List, Continue watching and Watched marks, tied to their Jellyfin account.
- Profiles inside Jellyfin: tap your avatar for "Who's watching?", pick a profile (its PIN if it has one) and the page switches to it, no signing out. Add a profile right there too. A new profile can see no more than the profile that made it, so a kids' profile can't make an unlimited one.
- The main profile asks for its Jellyfin password the first time a phone or browser switches into it, then remembers that device for 180 days. Settings > Jellyfin & streaming > Forget browsers makes them all ask again (a lost phone, a lent laptop).
- Made for phones and tablets too: a bottom tab bar, nothing cut off at the sides, bigger buttons, swipe through the top picks, and a player that works by touch. In the Jellyfin iPhone and Android apps the back gesture works inside Chillarr, and the player fills the screen.
- Jellyfin's own pages (sign-in, "Who's watching?", your settings and the dashboard) got Chillarr's dark look and font all the way through.
- Who may do what comes from Jellyfin: anyone signed in can watch, download and save to My List. Deleting files and stopping downloads needs Jellyfin's "Allow media deletion". Storage and library upkeep is for Jellyfin admins. Fixing apps, installing and changing folders stays in the Chillarr app on the PC.
- On LG TVs (Jellyfin's webOS app) Chillarr is the whole app, remote and all: arrows and OK everywhere, Back steps out (and leaves the app from Home), play, pause, stop and skip keys work in the player, and the number keys type a profile's PIN. Big posters, a clear highlight on whatever is picked, and lighter effects so older TVs don't lag. Older LG TVs (from 2018 on) get a version of the pages made for their browser, by themselves.
- Pictures keep their shape on every screen: films play letterboxed instead of stretched (wide, old 4:3 and phone-shot vertical video alike), subtitles stay on the picture, and posters and backgrounds crop instead of squashing, on TVs, phones turned either way, tablets and PCs.
- Starting and stopping something on the Discover shelf before it has downloaded no longer marks it watched. Jellyfin can't tell how long those are, so it took any stop as "watched to the end", and once the film arrived it showed under Completed (and the clean-up could have deleted it). Chillarr now takes that mark back within 30 seconds; a real watched mark on something you have is never touched.
- Phones and browsers keep their own names in Jellyfin's device list (an iPhone used to show up as "Windows").
- Removing a title from your Watch now list is allowed for everyone, even without Jellyfin's "Allow media deletion": it only frees what your own Watch now downloaded and nobody else uses.
- Switching profiles inside Jellyfin uses Jellyfin's Quick Connect. If it's off, Chillarr turns it on the first time someone opens the profile picker (it was already on here).
- TV apps that don't use Jellyfin's web pages (Android TV, Roku, Apple TV) keep Jellyfin's own look.
- Works on Jellyfin's http address (like http://your-pc:8096 or your Tailscale name). An https address in front of Jellyfin isn't supported yet.

## 0.8.3 (2026-10-21)
- The buttons on a hover card now do what they say instead of opening the title's page. Resume and Play start playing (a show plays the episode you're at), and Download on a film starts downloading straight away, in the quality you last picked. Download on a show still opens its page, so you can pick which episodes.

## 0.8.2 (2026-10-21)
- Clicking just below the progress bar (between the buttons) no longer pauses the film. Only a click on the picture itself does.
- The bars stay up while your mouse is on them, like YouTube, instead of fading after 3 seconds. Any mouse move brings them back.
- Skip and volume notes are bigger and stay up for 3 seconds. Skips keep adding up while the note is showing ("Forward 30 seconds", with where you are now), and the volume note has a level bar like VLC's.

## 0.8.1 (2026-10-20)
- Subtitles now line up with the film by themselves. Chillarr listens to the film's sound once and moves the subtitles to match, even ones made for another copy of the film (like Replicas, where one file was 36 seconds late and another was made for a 25 fps copy). Files next to the video are fixed on disk for good, so Jellyfin on the TV gets the fixed timing too. Subtitles Chillarr downloads by itself come already fixed.
- Picking a newly downloaded subtitle now really shows it. Before, the player could keep showing the old file after a download.
- Fix subtitle timing by hand: Timing - and + in the subtitles menu, or G and H while watching (like VLC). There's also a Sync to the audio button. Chillarr remembers your timing for that file.
- Full screen no longer loses the mouse: moving it brings the controls back, like it does in a window.
- Click anywhere on the picture to pause or play, with a big pause or play sign in the middle like Jellyfin's. Double click for full screen.
- Skipping says how far you went ("+30 s" when you press it three times), and Skip intro says how much it skipped. Volume changes show on screen, like VLC.
- Volume goes up to 300%, like VLC, without crackling. Use the slider, the arrow keys or the mouse wheel. M mutes. Your volume is kept for next time.
- Hovering the progress bar shows the time you'd jump to, with a little picture of that moment like in Jellyfin. Chillarr turns Jellyfin's preview pictures on for your Movies, Series and Anime; Jellyfin makes them in the background, so they show up film by film. The number keys jump to 10%, 20% and so on, and J and L skip back and forward.
- Night mode (in the subtitles and audio menu, or N): quiet talking gets louder and explosions softer, so you don't have to keep changing the volume.
- Sound delay, in the same menu: when voices come before the lips move, push the sound later. Chillarr remembers it for that title.
- Playing something you stopped part way asks "Resume from 42:10" or "Start over", like Jellyfin. A movie's own Resume and Start over buttons don't ask again.
- Esc closes the menu first, then full screen, then the player.
- A movie you searched for and then downloaded (like Replicas) no longer also shows up in Jellyfin's Discover shelf, so the TV doesn't list it twice.

## 0.8.0 (2026-10-20)
- Filters for Movies and Series now go as deep as Anime's. Movies: language, out now or coming soon, age rating, length and studio. Series: language, still running or ended, episode length, kind of show, network or streaming service, and country.
- Korean in one click: the new Language row on Filters shows only Korean shows (by language or by a Korean network), or leaves them out with a second click. Japanese, Chinese, Thai and Turkish work the same way. The new details fill in over the next day, so some titles show up later.
- The search box shows your recent searches in Chillarr's own look, one list per profile, instead of Windows' plain white box. Remove one with the x, or clear them all.
- Anime that isn't downloaded yet (like Bleach) now lists its episodes and offers Watch now.
- Stop and Remove buttons are as wide as the poster above them.
- Profiles with a PIN are properly locked. Changing or removing a locked profile needs its PIN. Forgot it? Delete the profile with your Chillarr login. Wrong PINs make you wait a little longer each time, and Chillarr opens locked until you pick a profile.
- Sonarr, Radarr and Prowlarr ask for your login again, also on your home Wi-Fi. Before, anyone on your Wi-Fi could open them without a password.
- The phone remote has a new, longer code. Pair your phone once more with the code in Settings. Wrong codes only lock out the device typing them, not your phone.
- Before letting apps past Defender's protected-folder rule, Chillarr asks first and names them. Turn the question back on in Settings > General.
- Plugins and helpers come from fixed, checked versions only (Intro Skipper, Open Subtitles, FlareSolverr), and Chillarr checks its own files before running them.
- Your Jellyfin sign-in and profile sign-ins are now saved encrypted, so other Windows accounts or a copied file can't read them.

## 0.7.7 (2026-10-19)
- New rows on the Anime page: Ecchi, Harem, Supernatural and School, next to Action, Romance, Comedy and Fantasy. Harem only shows shows that really are harem ones (High School DxD, Quintessential Quintuplets, Nisekoi), not every show with a hint of it.
- Opening Jellyfin at localhost:8096 in a browser now shows Chillarr's look and search, the same as the LAN and Tailscale addresses. A browser that had opened it before kept showing its old saved copies of Jellyfin's logo and search page. It now refreshes them once, by itself.

## 0.7.6 (2026-10-19)
- Skip dubbed copies now catches exactly the dubbed ones (like Hindi + English or Italian + English) and nothing else. In 0.7.5 it could flag plain English copies too.

## 0.7.5 (2026-10-19)
- New setting, on by default: Skip dubbed copies. Downloads leave out copies with an extra dubbed language mixed in (like "Dual Audio Hindi English"), so nothing starts in the wrong language. A film's own language is always fine, and so is anime with Japanese and English.
- Skip full-disc copies now also skips oversized normal copies of movies: about 8 GB at most for a two-hour film in 1080p, 20 GB in 4K. Whole seasons are judged per episode, so season packs still come.

## 0.7.4 (2026-10-19)
- New setting, on by default: Skip full-disc copies. Downloads never take a remux or a whole Blu-ray disc (25 to 80 GB for one film, looks the same on a TV). They take a normal copy of a few GB instead, everywhere: Download, Watch now and requests from the TV. Turn it off in Settings > General to allow remuxes again.
- Films you already have are left alone.

## 0.7.3 (2026-10-19)
- Next episode and Up next always offer the show's real next episode. If it isn't downloaded yet, it plays as it downloads ("Downloads as you watch"), and only that episode downloads. It crosses into the next season, and hides when the next episode isn't out yet.
- Back takes you to exactly where you were: the same page, the same spot on it, the same row position and the card you opened. Closing the player or a download finishing no longer throws you back to the top.
- The Back buttons on Explore and Filters, and the controller's B, return to where you were too.
- When Jellyfin's new look is switched on, Chillarr waits for Jellyfin to finish starting before checking it worked, so it no longer reports a false problem.

## 0.7.2 (2026-10-19)
- Jellyfin now looks like Chillarr in the browser and the phone apps: our colours, font and logo, rounded cards that lift on hover, a cinematic page for each title, a matching sign-in page and player. It's still a normal Jellyfin server, so every Jellyfin app finds it. (The Android TV app draws its own screens and keeps its look.)
- Searching in Jellyfin always searches everything. Searching from inside Movies used to miss titles on the Discover shelf (Iron Man) until you pressed "Retry with a global search".
- The first time this version runs, Chillarr restarts Jellyfin once to switch the new look on, only when nobody is watching.
- Chillarr's Watch now library no longer shows up in Jellyfin's menu and home page.
- Loading looks much better: the page shows the shape of what's coming (the big banner with its title and buttons, rows of posters across the whole width) with a soft light passing over it, instead of a flat grey block.
- Watch now on a show opens its episode list so you pick the episode. Every show lists its episodes, even ones you don't have yet.
- Watch now no longer fails with "error 400" while Jellyfin is busy scanning, and starts much faster (seconds instead of a minute of waiting on Jellyfin).
- Tokyo Toshokan works again: it now sits behind Cloudflare, so it goes through FlareSolverr. Any site that needs FlareSolverr gets it automatically, also ones added earlier.
- The Pirate Bay no longer shows as failing in Radarr. Its movie feed is always empty, which Radarr's test counted as a failure, and every Fix everything made Radarr skip it for longer. Searching it works and is kept.

## 0.7.1 (2026-10-19)
- On your TV or phone, shows on the Discover shelf now list their real episodes. Playing one gets only that episode, for series and anime alike. The show stays on the shelf so you can keep going episode by episode, and episodes you already have drop off it.
- Playing an episode of a show you stopped earlier gets just that episode, nothing it wanted before.
- The first start after updating takes a while in Jellyfin: it adds every shelf episode once.

## 0.7.0 (2026-10-19)

### Search
- Searching in Jellyfin's own apps (TV, phone) now finds what you type. Titles you search for that aren't on the Discover shelf yet are added within seconds, and the search index updates right after every shelf change instead of hours later.
- When one movie of a series is on the shelf, the rest of the series comes with it (Iron Man brings Iron Man 2 and 3).
- A title you stopped earlier goes back on the shelf instead of disappearing from Jellyfin.
- Search in Chillarr shows results as they come in: your own titles first, then everything else, with no waiting on the slowest source.
- A new search page: the top result's picture in the background, a title count, and Movies / Series / Anime chips.
- Every result shows its genres (High School DxD no longer says "No genre").

### Downloads
- Picking some seasons or episodes gets only those. New seasons don't start downloading by themselves.
- Anything you delete stays deleted. Sonarr and Radarr no longer download it again.
- Things that aren't out yet are searched for when they come out, then again after 1, 3, 6 and 12 hours, then daily, until a copy is found. This keeps going across restarts.

### Subtitles
- Download has a Subtitles choice per title, in any of 43 languages. English is picked for you when the title isn't in English.
- Subtitles are fetched when the file arrives, and looked for again for two weeks if none exist yet. They come from OpenSubtitles when you sign in (Settings), otherwise from free sources.
- Deleting a title or episodes deletes their subtitles too.

### My List > Completed
- Everything you finish shows up, counted per episode and season, including things you streamed before downloading.
- Each title says when it gets cleaned up ("Deletes in 2 days"), or "Deleted" once it's gone. It stays on the list after that, so you know you watched it.
- Titles you finished show "Watched" everywhere, and Download asks before getting one again.
- Taking a title off Completed only lasts until you finish more of it.

### Browsing
- Pressing the big banner at the top opens that title.
- "Similar to this" at the bottom of every title, from real recommendations, and it stays up to date. Click one to open it, Back to return.
- A new hover card: squarer, with a bigger title, genres and a smoother entrance.
- Titles without a poster get a proper card instead of a grey box.
- Filters explain what Season means for anime, with a Year next to it ("Fall 2025").
- Smoother animations across the app: loading, pages, the detail panel, toasts.

### Full screen
- Full screen now fills the whole monitor, not just the Chillarr window. In the player, and for the whole app with F11 or the new button in the top bar.

### Setup
- Setup has a new, optional Subtitles step: sign in to OpenSubtitles (or make a free account from there) so missing subtitles can be found, from Chillarr and from your TV or phone.
- The OpenSubtitles sign-in starts with your Chillarr name filled in.
