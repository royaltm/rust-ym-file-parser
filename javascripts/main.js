AyPlayer.init().then(AyPlayerHandle => {
  function $(e) { return document.getElementById(e) }
  var choice = $("ym-choice");
  var pause = $("ym-pause");
  var volume = $("ym-volume");
  var author = $("ym-author");
  var misc = $("ym-misc");
  var analyser = null;
  var analyserData = null;
  var oscilloscope = $("oscilloscope");
  var oscCtx = oscilloscope.getContext("2d");

  var ay = null;
  choice.addEventListener("change", function(event) {
    setElemText(pause, "⏸");
    if (ay) {
      ay.free();
      ay = null;
      analyser = null;
      clearOsc();
    }
    else {
      requestAnimationFrame(drawOsc);
    }
    setElemText(author, "-");
    setElemText(misc, "-");
    var ymurl = event.target.value;
    if (ymurl) {
      ay = new AyPlayerHandle(0.125);
      analyser = ay.createAnalyser();
      analyser.fftSize = 2048;
      analyserData = new Uint8Array(analyser.frequencyBinCount);
      ay.connectAnalyser(analyser);
      ay.setGain(volume.value);
      ay.load(ymurl).then(info => {
        setElemText(author, info.author);
        setElemText(misc, info.misc);
        ay.play(0);
      })
    }
  }, false);

  pause.addEventListener("click", function(event) {
    if (ay) {
      ay.togglePause().then(function(paused) {
        setElemText(pause, paused ? "⏯" : "⏸");
      });
    }
  }, false);

  volume.addEventListener("change", function(event) {
    ay && ay.setGain(event.target.value)
  }, false);

  function setElemText(elem, text) {
    clearNode(elem);
    elem.appendChild(document.createTextNode(String(text || "")));
  }

  function clearNode(node) {
    while (node.firstChild) {
      node.removeChild(node.firstChild);
    }
  }

  function populateSongs(select, songs) {
    clearNode(select);
    if (Array.isArray(songs)) {
      for (let i = 0, numSongs = songs.length; i < numSongs; i++) {
        let option = document.createElement("option");
        let name = songs[i];
        option.value = name ? "ymfiles/" + encodeURIComponent(name) + ".ym" : "";
        option.text = name || "(none)";
        select.appendChild(option);
      }
    }
  }

  function resizeOsc() {
    const { width, height } = oscilloscope;
    const { innerHeight, innerWidth } = window;
    if (innerHeight != height || innerWidth != width) {
      oscilloscope.height = innerHeight;
      oscilloscope.width = innerWidth;
    }
  }

  function clearOsc() {
    const { width, height } = oscilloscope;
    oscCtx.fillStyle = "#151515";
    oscCtx.fillRect(0, 0, width, height);
  }

  function drawOsc() {
    if (!analyser) return;
    requestAnimationFrame(drawOsc);
    resizeOsc();
    const { width, height } = oscilloscope;

    analyser.getByteTimeDomainData(analyserData);
    const bufferLength = analyserData.length;
    const sliceWidth = width / bufferLength;

    clearOsc();
    oscCtx.lineWidth = 1;
    oscCtx.strokeStyle = "rgb(0, 180, 240)";
    oscCtx.beginPath();

    for (let x = 0, i = 0; i < bufferLength; ++i) {

      let y = analyserData[i] / 255.0 * height;

      if (i === 0) {
        oscCtx.moveTo(x, y);
      } else {
        oscCtx.lineTo(x, y);
      }
      x += sliceWidth;
    }

    oscCtx.lineTo(width, height / 2);
    oscCtx.stroke();
  }

  const songs = "\
|150mph\
|Ace 2\
|Androids\
|Best Part of The Creation\
|Call me\
|Commando Highscore\
|For Abyss\
|Gauntlet 3\
|Ghouls 1\
|Ghouls 2\
|Ghouls 3\
|Iceage (digi)\
|Ik+\
|Led Storm 1\
|Led Storm 2\
|Led Storm 3\
|Lightforce\
|Master of magic\
|Monty Highscore\
|ND-Credits\
|ND-Toxygene\
|Powerman\
|Prelude\
|Rise\
|Sanxion\
|Scramble Sim\
|Seagulls\
|Sharpness Buzztone\
|Sid Music #1\
|Sid Music #2\
|Spellbound\
|Stars\
|Steps\
|Sunrider\
|Terramex\
|The Arrival\
|The Cave\
|The Dancer\
|The Delegate (jackit)\
|Walk on Ice\
|Wide Awake\
|Zoolook\
".split("|");
  populateSongs(choice, songs)
});
