# Changelog

## 0.14.0 (2026-11-08)
- New: Style in Movies too. Cinema, Midnight or Classic, each in dark or light, from the picker in the middle of the top bar. Movies and reading each keep their own style, for each profile.
  - Midnight: dark violet and dense, a shorter picture at the top, ratings on the covers, and a Popular ranking down the side (Top 10, Trending, Series).
  - Classic: the featured picture is a card inside the page, and the Top 10 is a numbered chart with the title and genre beside each cover.
- New: Explore all on reading rows. Click a row's title (Popular manhwa, Fantasy books and the rest) to see everything in it, with more loading as you scroll. Book rows too.
- Reading looks tidier:
  - The arrows on the big picture sit with the dots at the bottom right, so they never cover the title. Same in Movies.
  - The picture behind is sharper when the series has a banner.
  - New chapters is only as wide as what's in it, not a big empty panel.
  - Midnight: the Popular now grid lines up with its heading, and Latest updates is a proper grid.
  - Classic: the Top 10 is a numbered chart with small covers. Cards show the title, then the genre and length on one line. UP and NEW no longer sit on top of each other.
  - Lists that switch with tabs fade across instead of jumping.
- Light mode, everywhere: buttons, the details panel, the series panel and the hover cards no longer keep dark colours. The player and the reader stay dark.
- On a narrow window the Style picker sits at the top of the page, in one row.
- New: 18+ titles can also come from HentaiRead. It's only searched for 18+ titles.
- 18+ in the reading filters can be turned on from any profile (it's still off until you pick it).
- A TV, phone or browser signed in as a profile with a PIN now asks for that PIN first.
- Each profile has its own manga and books: what one follows doesn't show on another's, and following something already here doesn't download it again.
- A profile made on a TV or phone now shows in the PC's profile picker right away.
- Watch together no longer micro-stutters: tiny pauses aren't sent to the whole party any more, and small drifts are caught up gently instead of jumping.
- Fixed: a Watch now film could stay in Downloads after it finished (The Dark Knight did). A player that has said nothing for 10 minutes no longer counts as still watching.

## 0.13.4 (2026-11-08)
- New chapters are hard to miss now: a glowing New chapters section at the top of reading Home (in every style), a count on the Home tab, NEW on each new chapter in a series, and a short note when you open reading. New means out since you last read that series, so following something doesn't call its whole back catalogue new.
- Eight more manga sites: Thunder Scans and Manhua Plus (furthest ahead on long manhua: The Devil Butler goes to chapter 922 in English there, where the others stopped at 701), plus ManhuaUS, Top Manhua, Mangakakalot, Mangabat, MangaK and Zinmanga. They're added the next time the manga downloader starts. On a series you follow, Change copy lists them.
- New: Dark and Light for every reading style (the sun and moon next to Style). Each style remembers its own; Classic starts light, the others dark.
- New: Similar to this, under every series.
- Filters: a Themes row next to Genres (Isekai, Murim, Regression, Cultivation, Villainess and more), and 70 themes under More filters. AniList itself has 18 manga genres.
- The reading tabs take on your style: Midnight gets the ranking down the side, Classic a banner and a numbered top 5.
- Fixed: Midnight's smaller picture at the top went under the top bar on wide screens.
- Fixed: Classic's hover cards were washed out (light grey on white).

## 0.13.3 (2026-11-08)
- Manhwa, Manhua, Manga and Novels now open with the big featured picture too, showing that tab's trending series, with your filters and the grid under it.
- The Style picker shows on every reading tab, not only after visiting Home.
- The top bar's buttons move now: the film and book switch wiggles, Watch together's people hop, Full screen's corners spread, the Settings gear turns, and every one lifts on hover and gives a little when pressed. The People button no longer tips sideways on hover.

## 0.13.2 (2026-11-08)
- New: Style, in the middle of the top bar while reading. Pick how reading looks:
  - Cinema: big pictures and rows, like the films. Trending is now a Top 10 with the big numbers.
  - Midnight: dark violet and dense. The newest chapters of what you follow up front, each one a click from the reader, with how long ago it came out; a grid of what's popular with ratings and chapter counts; and a Popular ranking down the side (Trending, Top rated, Most read).
  - Classic: light and bright. A numbered Top 10 (Trending, Popular, Top rated), Your week (your series by the day their chapters come out, with UP on today's), Popular by genre, and New and ongoing.
  Each profile keeps its own style.
- New: hover cards on manga, manhwa and manhua covers, like the films': the cover opens out with the picture, rating, a few lines, Read or See chapters, and My List.
- New: filters on Manhwa, Manhua, Manga and Novels like the films': genres and tags in or out, status, rating, length and years. 18+ can be hidden, included or shown alone, only on the PC owner's profile (or a Jellyfin admin on the web); other profiles never see 18+ titles.
- More movement: covers rise in as they load, rankings and updates slide in, switching style fades across, and the tabs and buttons react when pressed.

## 0.13.1 (2026-11-08)
- New: Keep up to date, on any series you follow. Chillarr checks the site for new chapters every few minutes instead of every few hours. A site that has trouble is asked less often until it answers again.
- New: a notification on this PC when new chapters are ready to read, for every series you follow, and when another site gets further ahead than your copy (Change copy on the series switches to it).
- New: mark chapters read in one go: chapters X to Y, or the whole series. Marking moves your place forward too.
- New: delete a series when it's finished. Settings > Library & storage > Delete chapters after reading now has As you read or When finished: the whole series goes a few days after everyone reading it has read the last chapter.
- New: the reading pages open with a big featured series at the top, like Movies, with the cover tilted on the right.
- A series you don't follow shows its chapters as soon as you open it, with nothing ticked. Tick the ones you want, press Download all, or press Follow at the top to just get new ones.
- Following a series again picks the copy whose chapters you kept, so they're there to read straight away.
- The series page opens more smoothly, and the reader's bars are more solid over white pages.
- Sharpen pages now stops the moment a game goes full screen, instead of finishing the chapter it was on.
- Fixed: after the clean-up deleted chapters you'd read, new-chapter checks could download every chapter you hadn't picked.

## 0.13.0 (2026-11-08)
- New: Reading mode. The book button at the top switches Chillarr to reading, with its own tabs (Home, Manhwa, Manhua, Manga, Novels, Books, Downloads, My List) and its own search. The film button switches back. Each profile remembers which one it was in.
- New: pick the translation you want, then Best or a copy of your own choosing.
- New: pick which chapters to get, or get one chapter at a time from the series page. You can also choose whether new chapters come in by themselves.
- Best now mixes copies: chapters come from the sharpest copy, and the newest ones from whichever site is furthest ahead. You get the good quality without waiting on a site that locks its newest chapters.
- Manga downloads now stay inside Limit downloads, like everything else.
- The reader zooms in and out in every mode: the - and + at the top, the + and - keys, Ctrl and the mouse wheel, or a pinch. Each profile keeps its own zoom for each mode. Scroll now starts at the pages' real size instead of blowing them up.
- The reader is smoother: pages slide in the way the story reads, switching modes or chapters fades across, and the arrow keys scroll smoothly.
- New: Sharpen pages with your graphics card, in Settings > General. Each chapter you download is redrawn once at twice the detail, in the background, and the reader shows HD on those. It waits while a game is full screen. Turning it off deletes the sharper copies; your chapters stay. It only shows in the app on this PC.

## 0.12.1 (2026-11-08)
- Fixed: pressing Find copies while the manga downloader was still being set up could say it "didn't start". It now waits for the setup to finish.

## 0.12.0 (2026-11-08)
- New: Read. A new tab for manga, manhwa, manhua, books and light novels, on the PC, phones and the TV.
- Pick a series and Chillarr checks every site for it, then shows the copies sharpest and most complete first. Download one and every chapter comes in, new chapters too.
- The reader shows each page exactly as the site has it. Scroll for manhwa and manhua (one long strip, the next chapter follows on), Right to left for manga (two pages side by side on a wide screen), and Left to right.
- Books and light novels come through your download sites like films do. Books open in a reader with Dark, Sepia and Light and a text size, and PDFs open in your browser.
- Each profile keeps its own place, read chapters and Finished list. Settings > Library & storage can delete chapters a few days after everyone reading them has read them.
- The manga downloader is set up for you, hidden like the other apps. New installs get it during setup, and this PC gets it the first time Chillarr starts after updating.
- New: Comments on every title. Everyone on your server sees them, and you can edit or delete your own.
- New: Chat in Watch together. Press C, or the chat button, to talk while you watch. The video never pauses.
- New: Ecchi works as a genre for films and shows from your library too, and genres are listed A to Z everywhere.
- Fixed: the first sign-in could open a profile with a PIN. A locked profile now always asks who's watching.
- Fixed: downloads no longer go into your own Downloads folder. They go to a Chillarr Downloads folder on the same drive as your library, and what was already downloading moves there too.
- Fixed: a finished download left behind after its title was removed (Meet Joe Black) is cleaned up.
- Fixed: Play on a show now starts the episode you have, not the first one of the show. Edgerunners started downloading season 1 again while you had season 2.

## 0.11.5 (2026-11-05)
- New: Use graphics card for video, in Settings > General. Video that has to be converted, like x265 films on a TV, is converted on your graphics card instead of the processor, which can help stutter on a TV and leaves more for your games. It only shows in the app on this PC.

## 0.11.4 (2026-11-05)
- New: Limit downloads, in Settings > General. All downloads together use at most 10% of your internet, up and down, so games on this PC stay smooth. It only shows in the app on this PC, never on a phone or TV.
- New, with it: Don't limit what I start here. Downloads you start in the app on this PC run at full speed, and ones started anywhere else stay limited.
- Pressing Watch now on something already downloading now switches that download to start-to-end order, so it plays as it comes instead of stopping at missing pieces.
- Watch now no longer sits on "Getting the first minutes" with most of the film downloaded. If the opening pieces get stuck on someone slow, Chillarr restarts the download so they come from someone else.
- Watch now opens faster: Jellyfin no longer spends minutes reading the download before it starts.
- If your phone loses touch with your PC while waiting, the screen now says so and keeps trying.
- On iPhones that won't start a video by themselves, the waiting screen now clears so you can press play.

## 0.11.3 (2026-11-05)

- Watch now no longer fails with "Jellyfin isn't answering". Jellyfin can't open a title until the start of it has downloaded, and Chillarr gave up after 20 seconds. It now stays on the waiting screen until the title can open, on every device.
- Jellyfin being slow now says it's busy, instead of saying it isn't answering.
- Night mode works on iPhone and on TVs. The first time you turn it on for a film, Chillarr makes an evened-out copy of the sound in a few minutes while you keep watching, then switches to it. Files you're still downloading get it once they're done.
- The subtitles and audio menu on a phone is now three short pages (Subtitles, Audio, Speed). Size and the other controls stay in view, and sideways it sits beside the picture.

## 0.11.2 (2026-11-04)
- Settings are much simpler. The everyday options come first, and technical ones are folded under More options or listed under Advanced. Nothing was taken away.
- System is now Status, and it just tells you if everything is working. The list of apps opens by itself if something breaks.
- Jellyfin & streaming is now Account & devices, with how to watch on your TV or phone near the top. Indexers is now Download sites.
- Explanations are shorter and use plainer words.
- OpenSubtitles is now a choice at the end of setup: Set it up or Skip, with a link to make a free account. You can still set it up later in Settings.

## 0.11.1 (2026-11-04)
- Tabs open at once. Each one shows what it had last time, even right after starting Chillarr, and updates what changed a moment later. Every tab is also made ready in the background shortly after starting.
- Everything loads faster: Chillarr asks your apps for things side by side instead of one after another, shares answers instead of asking for the same list over and over, and only works out what the tab you opened needs. Animations are quicker too, so nothing is held back from view.
- Fix everything no longer restarts Radarr every time. Radarr kept showing nekoBT and SubsPlease as failing because they're anime sites with no new movies to list, and restarting it for that made it slower each time, until it could stall for 15 seconds and show "Not running" while it was running. A restart now really closes and starts the app, and happens at most once in 6 hours.
- A log file that grew huge because of that (2 GB on one PC) is cleared by itself when Chillarr starts.
- Stopping a download clears "Downloading 0%" from its poster. It used to stay there until you restarted Chillarr.
- A site that's down, like YTS right now, no longer counts as something for you to fix. Its card says the site is down and your other sites still work, and Chillarr stops testing it every 15 minutes in the background. Those tests were keeping your apps busy and making the site's retry wait longer.
- An app that's busy says "Slow to answer" instead of "Not running".

## 0.11.0 (2026-11-03)
- Setting up a new PC is one screen and one press. Pick a folder, a login, Subtitles or Dubbed, and a colour, press Set up Chillarr, and it does the rest: every download, every install, starting the apps and connecting them.
- One big meter shows all of it: how much has downloaded out of the total, the speed, the time left, and a line for each app. Broken connections pick up where they stopped.
- Windows asks for permission once instead of once per app. That one prompt covers the installs, the firewall rules (no more "Allow access?" windows) and Defender's protected folders.
- Chillarr's own installer is no longer a wizard. Double-click it, a short progress window runs, and the app opens.
- Sites are added by themselves. If one is down or blocked, Chillarr tries again later without being asked, up to every 6 hours. One you take out of Prowlarr stays out.
- Choose original voices with subtitles, or Dubbed in the language you pick (English, Spanish, French, German, Japanese and more), in Settings > General. It decides which copy gets downloaded and which audio a title starts in, when the file has both. Copies with both voices are picked first when the quality is the same, so the player's audio button can switch between them.
- Pick your colour in Settings > Appearance: seven themes or any colour you like, and a pure black background. The whole app follows, and so do the logo, the tray icon and your desktop and taskbar shortcuts.
- New hover cards: the poster stays under your mouse and the details open beside it, with the trailer above them.
- Series and anime show what you've watched clearly: a Watched label on each episode, a count for each season, and buttons to mark an episode or a whole season as watched or not watched. Marks stay after the files are deleted, even right after you finish an episode.
- Only Space or the pause button pauses now. Clicking the picture just brings the controls back. TV remotes keep their OK and play buttons.
- One icon in the tray: Chillarr's. Sonarr, Radarr and qBittorrent run without their own. The tray menu has "Stop everything and quit" for closing them all.
- Fix everything is faster, since apps start and get checked side by side, and it shows each step as it goes, then what it changed and how long it took.
- Starting up is quicker. The window opens straight away instead of waiting for every app to answer, and your apps start the moment Chillarr does instead of 30 seconds later, side by side.
- Apps that wouldn't start: if an app can't open its own files, Fix everything repairs that with one Windows prompt. One stuck without answering gets restarted, and if an app still won't start, Logs shows the app's own error instead of only "didn't answer".
- Prowlarr no longer runs as a Windows service. If it stopped, say for an update, every search failed until someone pressed Fix everything and answered a Windows prompt. Now Chillarr just starts it again, and moving all the apps over takes one prompt.
- Watch now on a profile limited to certain libraries no longer stops at "Jellyfin answered with error 404": it's let into Watch now's hidden library.
- Settings > Logs has a Save logs button: one file with what Chillarr did, how each app is doing, and each app's own log, with keys and passwords hidden. Chillarr's log also records every error Jellyfin, Sonarr, Radarr and Prowlarr answer with, and each step of Watch now.
- Adding several sites no longer makes Prowlarr's sync clash with itself in Sonarr and Radarr.
- Watch now: pressing Back while it's still looking for a copy really cancels, and a film asked for that way doesn't download later anyway. Removing a title mid-start keeps it removed.
- Watch now: stopping partway no longer marks it watched, your place is no longer lost when you leave during a reload or right after Start over, a paused season pack starts again for the episode you're on, Next stays full screen, the spinner clears after a seek while paused, and a stalled stream gives up with a message instead of spinning forever.
- The status dots on the app cards in Settings show the right colour. They were always grey.

## 0.10.0 (2026-10-30)
- Switching tabs is instant. Going back to Movies, Anime or any tab you've already opened shows it straight away, scrolled where you left it, and refreshes it quietly in the background. Only what actually changed gets redrawn.
- Remove from Continue watching: a title you won't finish can be taken off the row from its page or its card.
- Press ? in the player (or the new button in its bar) for a list of every shortcut.
- Seeking with the arrow keys, a TV remote or a controller now shows the preview picture too, not only with the mouse.
- The audio language you pick carries on to the next episode, the way subtitles already did.
- Each profile keeps its own player settings: subtitles, volume, night mode, quality and sound delay.
- "Skip full-disc copies" no longer moves a quality profile's target. A profile that allowed 4K but aimed for 1080p was switched to aim for 4K, so Radarr started upgrading films nobody asked to upgrade.
- Stopping a Discover title partway no longer counts as watching it, even if its download finished while you watched. Before, the clean-up could delete it as finished.
- A show stopped right after its download started stays stopped instead of being downloaded again.
- Finished Watch now downloads give their disk space back.
- Fix everything no longer force-closes a qBittorrent that is still starting or uses another port, and when it does need a restart it closes it properly first. A settings file it can't read is left alone instead of being replaced by an almost empty one, and download folders with a comma in their name work.
- Blocked indexers (like 1337x or EZTV) now really try their other addresses before giving up.
- Anime filters by tag show titles that are really about that tag, like the tag rows do. Chillarr also stays under AniList's limit, so anime rows stop failing to load in busy moments.
- Trickplay turns on for libraries added later, not only the ones there on the first run.
- The clean-up says so when a file couldn't be deleted, instead of reporting it as cleaned.
- Settings, profiles, history and the Discover shelf are saved in a way a crash or power cut can't damage. If the profiles file is lost anyway, Chillarr opens locked and asks for the main profile's Jellyfin sign-in instead of dropping every PIN. PIN lockouts now survive a restart.
- Safer inside Jellyfin: someone signed in on the web can only delete titles they can see, and the phone remote searches as whoever is signed in on that TV, never with the PC owner's account. Each Jellyfin account can have up to 5 phone-remote screens, and one device can no longer use up every connection and lock the others out.
- On LG TVs: the big banner on Home opens its title, search backgrounds show, picking a Similar title keeps its buttons, subtitles look clean, and Chillarr no longer slowly uses more memory the longer it stays open.
- Buttons in Settings no longer get stuck on "Testing..." or stay greyed out after something fails, and a page that can't load shows Try again instead of loading forever.
- The player no longer doubles its progress reports when you switch tracks or episodes quickly, and the TV remote's Back closes an open menu over the player before the player itself.

## 0.9.2 (2026-10-28)
- My downloads no longer says "Nothing downloaded yet" when you do have things. If Jellyfin was still starting (it takes about half a minute) or too busy to answer, the page took that as an empty library. Now it says it's getting your library and keeps trying until Jellyfin answers.

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
