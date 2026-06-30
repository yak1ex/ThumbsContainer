## Objectives

### Main objectives

* We want to develop a tool to organize media files, such as images and videos, including archive files of them.
  * Typical use cases are as follows: First, register media files under the specified folders in database. Then, users add information for the files. Users search media files by the information.
  * Provide a feature to help dedupe of the same files. This MUST not automatically done. Decision of removal should be made by user.

### Details

#### Target files

* Target media files are treated as containers of image files.
  * A video is a time series container of images.
  * An archive of image files is a container of the containing image files.
  * Images files need special handling. If a folder contains image files only and does not have sub folders, the folder is considered as a container. Otherwise, image files in a filer is grouped and the group is consiered as a virutal container.
* Containers can be combined as a new container. In search function, described at later, provide an option to hide containers in combined containers.
* Each container is related to corresponding thumbnails and user-defined classifications.
  * User-defined classifications can be added later by users themselves.
    * User-defined classifications mainly consist from 2 parts. One is textual description, that is tag, and another is rating for various perspecitves.
* The following file types should be supported. It is not limited to them. Try file type implied by its extension, auto-detect if failed.
  * Video files: mp4, mkv, avi
  * Image files: jpg, png, gif, webp, avif
  * Archive files: rar, 7z, zip, lzh

#### Register files

* There are automatic registering and manual regisetring.
  * Automatic registering is triggered by users, then it collects hash, creation date and time, file name, path and file size.
    * Probably hash should be a part of primary key. Why hash itself is not a primary key is preparation for hash collision.
      * The tool can detect file moves. Corresponding information, except for path-specific information, should be maintained.
  * At manual registering, users add and edit tags and ratings during seeing existing information such as thumbnails.
    * All tags and ratings are optional and may be added at any time.

#### Search

* Users can search media files by combination of path, filename, rating, and tags.

## Development

* Develop step-by-step. At each step, an executable should be build, then ask me to check the executable.
* Use Rust + Typescript, Tauri and Svelte for frameworks.
* Use SQLite for database.
* You can assume FFmpeg executables `ffmpeg.exe` and `ffprobe.exe` are available in executable search PATH.
* Additional task: add sub-progress for large files during registration (per-file progress while hashing/processing).
