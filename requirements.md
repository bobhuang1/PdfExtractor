Need to write a PDF and docx/doc document extractor, with functions similar to docling. You can check GitHub docling project for details. Instead of calling other libraries and scripts and packages, we need to write everything from the ground up (including OCR, learn from tasseract and write our own code)

Lanaguage: Rust, need to be able to conert documents into same docling JSON format. 

Should have a standard command line (CLI) interface to allow file path as one of the input parameters, the other input parameters should follow normal Linux conventions. Outputpath should also be included as one of the parameters, if not provided, output in the same folder as input.

UI: simple, one file selection (file or directory), one settings. It acts as a called to the CLI function.

Platform: need to run Windows, Linux and MacOS, prefer a single code base.

Parallel conversion: should be able to convert multiple files at the same time and the parallelism is set in settings from 1 to 1000.

Questions:
1. Is RUST a good choice?
2. Is it possible to utilize a GPU if detected?
3. What is the best way to do OCR? Is there a good and free library we can borrow natively in RUST?
4. What's your overall recommendation for this project?


