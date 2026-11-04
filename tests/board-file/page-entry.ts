// What the board file specs load into the page: the real session and a fake daemon that follows the interface note.
import { createBoardFilePartJoiner, createBoardFileSession } from '../../src/renderer/lib/board-file/board-file-session'
import { createFakeBoardDaemon } from '../../src/renderer/lib/board-file/fake-board-daemon'

;(window as unknown as { BF: unknown }).BF = { createBoardFileSession, createBoardFilePartJoiner, createFakeBoardDaemon }
